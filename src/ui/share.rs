//! Tailscale sharing.
//! Owner: put the SSH machines on the tailnet, lock guests to SSH on them, invite people.
//! Guest: paste an invite code to add those machines (reached via their Tailscale addresses).
use super::*;
use crate::backend::{Overview, PlanArgs, PlanView, TsStatus};
use crate::tailscale::{InviteMachine, Peer, SparkArg};

#[derive(Default)]
pub struct Share {
    ts: Option<TsStatus>,
    overview: Option<Overview>,
    busy: bool,
    /// (email, message, code) of the invite just created.
    result: Option<(String, String, String)>,
    plan: Option<PlanView>,
    error: String,
    token: String,
    email: String,
    code: String,
}

/// Per-guest accounts: run the account step on every shared machine in a visible tab (it needs sudo, so the owner
/// types their password there). The script is idempotent; userdel is never run: a removed guest's account is locked
/// and their processes killed, their home folder stays for the owner to inspect or delete.
fn useradd_cmd(user: &str, email: &str) -> String {
    format!(concat!(
        "u={u}; if id -u \"$u\" >/dev/null 2>&1; then echo \"account $u exists\"; else sudo useradd -m -s /bin/bash -c {c} \"$u\" && echo \"created $u\"; fi; ",
        "sudo usermod -U \"$u\" 2>/dev/null; for g in video render docker; do getent group \"$g\" >/dev/null && sudo usermod -aG \"$g\" \"$u\"; done; ",
        "sudo chmod 750 \"/home/$u\" 2>/dev/null; sudo -u \"$u\" -H bash -lc 'mkdir -p ~/.nexus && chmod 700 ~/.nexus && command -v claude >/dev/null || echo \"note: claude is not on PATH for this account; it needs its own install or a shared one in /usr/local/bin\"'; echo done"),
        u = user, c = crate::remote::shq(&format!("Nexus guest {email}")))
}
fn userlock_cmd(user: &str) -> String {
    format!("u={user}; if id -u \"$u\" >/dev/null 2>&1; then sudo usermod -L \"$u\" && sudo pkill -u \"$u\"; echo \"locked $u (home folder kept)\"; else echo \"no account $u\"; fi")
}
fn valid_email(e: &str) -> bool {
    let Some((l, d)) = e.split_once('@') else { return false };
    !l.is_empty() && !e.chars().any(char::is_whitespace) && !d.contains('@') && d.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
}

impl App {
    fn ssh_machines(&self) -> Vec<Machine> { self.cfg.machines.iter().filter(|m| m.is_ssh()).cloned().collect() }
    fn peer_for(&self, m: &Machine, ts: &TsStatus) -> Option<Peer> {
        let hn = self.host_of(&m.id).to_lowercase();
        ts.local.peers.iter().find(|p| !hn.is_empty() && p.host_name.to_lowercase() == hn)
            .or_else(|| ts.local.peers.iter().find(|p| !m.ts_host.is_empty() && (p.dns_name == m.ts_host || p.ips.contains(&m.ts_host)))).cloned()
    }
    fn spark_args(&self) -> Vec<SparkArg> {
        self.ssh_machines().iter().filter(|m| !m.invited).map(|m| SparkArg { hostname: self.host_of(&m.id), ts_host: m.ts_host.clone() }).filter(|x| !x.hostname.is_empty() || !x.ts_host.is_empty()).collect()
    }
    fn ssh_users(&self) -> Vec<String> { let mut u: Vec<String> = self.ssh_machines().into_iter().map(|m| m.user).filter(|u| !u.is_empty()).collect(); u.sort(); u.dedup(); u }
    /// Per-guest Linux accounts (default for new setups); an existing setup keeps shared accounts until the owner switches.
    fn per_guest(&self) -> bool {
        self.cfg.tailscale.as_ref().and_then(|t| t.per_guest_users).unwrap_or_else(|| !self.share.as_ref().and_then(|s| s.overview.as_ref()).is_some_and(|o| !o.guests.is_empty()))
    }
    fn plan_args(&self, add: Option<String>, remove: Option<String>) -> PlanArgs {
        PlanArgs { machines: self.spark_args(), ssh_users: self.ssh_users(), add_guest: add, remove_guest: remove, per_guest_users: self.per_guest() }
    }

    pub fn open_share(&mut self) {
        self.menu = None;
        self.share = Some(Share::default());
        self.refresh_share();
    }
    fn close_share(&mut self) { self.share = None; self.focus_terminal(); }
    fn share_err(&mut self, e: String) { if let Some(s) = self.share.as_mut() { s.busy = false; } self.toast_bad(e); }

    fn refresh_share(&mut self) {
        let (be, args) = (self.be.clone(), self.spark_args());
        self.be.run(async move {
            let ts = be.ts_status().await;
            let ov = if ts.has_token { Some(be.ts_overview(args).await) } else { None };
            (ts, ov)
        }, |app, (ts, ov)| {
            // Remember each machine's tailnet address so Nexus can fall back to it away from the LAN.
            let mut changed = false;
            for m in app.ssh_machines() {
                if let Some(p) = app.peer_for(&m, &ts).filter(|p| !p.dns_name.is_empty() && p.dns_name != m.ts_host) {
                    if let Some(x) = app.cfg.machines.iter_mut().find(|x| x.id == m.id) { x.ts_host = p.dns_name; changed = true; }
                }
            }
            if changed { app.save_now(); }
            let Some(s) = app.share.as_mut() else { return };
            s.busy = false;
            s.ts = Some(ts);
            match ov { Some(Ok(o)) => { s.overview = Some(o); s.error.clear(); } Some(Err(e)) => { s.overview = None; s.error = e; } None => {} }
        });
    }

    /// Preview + apply a policy change, then `then` (skipped when the user declines).
    fn apply_plan(&mut self, add: Option<String>, remove: Option<String>, confirm: Option<String>, then: impl FnOnce(&mut App) + Send + 'static) {
        let (be, args) = (self.be.clone(), self.plan_args(add, remove));
        self.be.run(async move { be.ts_plan(args).await }, move |app, r| {
            let plan = match r { Ok(p) => p, Err(e) => { app.share_err(e); return; } };
            if plan.changes.is_empty() { then(app); return; }
            let apply = move |app: &mut App| {
                let be = app.be.clone();
                app.be.run(async move { be.ts_apply(plan.etag).await }, move |app, r| match r { Ok(()) => then(app), Err(e) => app.share_err(e) });
            };
            match confirm {
                None => apply(app),
                Some(text) => {
                    let desc = format!("{text}\n\n• {}", plan.changes.join("\n• "));
                    // The continuation crosses threads to get here but runs on the UI thread from now on.
                    let cell = std::cell::RefCell::new(Some(apply));
                    app.choose("Update your tailnet?", &desc, &[("Cancel", false), ("Apply", true)], move |app, v| {
                        if v == Some(1) { if let Some(f) = cell.borrow_mut().take() { f(app); } } else if let Some(s) = app.share.as_mut() { s.busy = false; }
                    });
                }
            }
        });
    }

    fn invite(&mut self, email: String) {
        if !valid_email(&email) { self.toast_bad("Enter the email address they use (or will use) to sign in to Tailscale"); return; }
        let sparks: Vec<Machine> = self.ssh_machines().into_iter().filter(|m| !m.ts_host.is_empty()).collect();
        if sparks.is_empty() { self.toast_bad("Put your machines on the tailnet first (step 2)"); return; }
        if let Some(s) = self.share.as_mut() { s.busy = true; }
        let e2 = email.clone();
        self.apply_plan(Some(email.clone()), None, Some(format!("Give {email} SSH access to your shared machines? This updates your tailnet:")), move |app| {
            let (be, e3) = (app.be.clone(), e2.clone());
            app.be.run(async move { be.ts_invite(e3).await }, move |app, r| {
                let url = match r { Ok(u) => u, Err(e) => { app.share_err(e); return; } };
                let guest_user = app.per_guest().then(|| crate::tailscale::guest_user(&e2));
                if let Some(u) = &guest_user {
                    for m in &sparks { app.launch(&m.id, "shell", Extra { raw: true, cmd: Some(useradd_cmd(u, &e2)), name: Some(format!("Account {u} → {}", m.name)), ..Default::default() }); }
                    app.toast(format!("Creating {u} on {} (sudo password in the new tab{})", sparks.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "), if sparks.len() > 1 { "s" } else { "" }), OK, 6000);
                }
                let tailnet = app.share.as_ref().and_then(|s| s.ts.as_ref()).and_then(|t| t.local.tailnet.clone());
                let machines: Vec<InviteMachine> = sparks.iter().map(|m| InviteMachine { name: m.name.clone(), host: m.ts_host.clone(), user: guest_user.clone().unwrap_or_else(|| m.user.clone()), icon: if app.machine_icon(m) == "nvidia" { "nvidia".into() } else { String::new() } }).collect();
                let code = crate::tailscale::encode_invite(tailnet.as_deref(), &machines);
                let users = machines.iter().map(|m| format!("{}: {}", m.name, m.user)).collect::<Vec<_>>().join(", ");
                let message = format!("You're invited to use my machines through Nexus.\n\n1. Install Tailscale (https://tailscale.com/download) and join my tailnet with this link, signing in as {e2}:\n   {url}\n\n2. Install Nexus: https://github.com/{}/releases\n\n3. In Nexus click the Share (people) icon, paste this invite code under \"Got an invite code?\", and click Add machines:\n   {code}\n\nYou'll log in as {users}. Please don't change other people's sessions or settings.", crate::backend::REPO);
                if let Some(s) = app.share.as_mut() { s.result = Some((e2.clone(), message, code)); s.email.clear(); }
                app.refresh_share();
            });
        });
    }

    fn remove_guest(&mut self, email: String) {
        let g = self.share.as_ref().and_then(|s| s.overview.as_ref()).and_then(|o| o.guests.iter().find(|g| g.email == email).cloned()).unwrap_or_default();
        let desc = format!("They lose SSH access to your machines immediately{}.", if g.user_id.is_some() { " and are removed from your tailnet (their devices too)" } else { "" });
        self.choose(&format!("Remove {email}?"), &desc, &[("Cancel", false), ("Remove", true)], move |app, v| {
            if v != Some(1) { return; }
            if let Some(s) = app.share.as_mut() { s.busy = true; }
            let (e2, g) = (email.clone(), g.clone());
            app.apply_plan(None, Some(email.clone()), None, move |app| {
                let be = app.be.clone();
                app.be.run(async move { be.ts_revoke(g.user_id, g.invite_id).await }, move |app, r| {
                    if let Err(e) = r { app.share_err(e); return; }
                    if app.per_guest() {
                        let u = crate::tailscale::guest_user(&e2);
                        for m in app.ssh_machines().into_iter().filter(|m| !m.ts_host.is_empty() && !m.invited) { app.launch(&m.id, "shell", Extra { raw: true, cmd: Some(userlock_cmd(&u)), name: Some(format!("Lock {u} → {}", m.name)), ..Default::default() }); }
                    }
                    app.toast_ok(format!("{e2} removed"));
                    app.refresh_share();
                });
            });
        });
    }

    fn join_with_code(&mut self, code: String) {
        let be = self.be.clone();
        self.be.run(async move { be.ts_decode_invite(code).await }, |app, r| {
            let inv = match r { Ok(i) => i, Err(e) => { app.share_err(e); return; } };
            let fresh: Vec<InviteMachine> = inv.machines.into_iter().filter(|x| !app.cfg.machines.iter().any(|m| m.host == x.host || m.ts_host == x.host)).collect();
            if fresh.is_empty() { app.toast_ok("Those machines are already in Nexus"); return; }
            // Show exactly where this code points before connecting: Nexus will log in there, run sessions there, and may
            // host team chat there. Only accept codes from someone you know, received over a channel you trust.
            let desc = format!("This invite{} adds: {}. Nexus will connect to them over Tailscale SSH without offering your SSH keys. Anything you type, paste or drop into their sessions goes to those machines. Only continue if you were expecting this invite.",
                if inv.tailnet.is_empty() { String::new() } else { format!(" (tailnet {})", inv.tailnet) }, fresh.iter().map(|x| format!("{} = {}@{}", x.name, if x.user.is_empty() { "?" } else { &x.user }, x.host)).collect::<Vec<_>>().join(", "));
            app.choose("Add these machines?", &desc, &[("Cancel", false), ("Add machines", true)], move |app, v| {
                if v != Some(1) { return; }
                for (i, x) in fresh.iter().enumerate() {
                    app.cfg.machines.push(Machine { id: format!("ts-{}{i}", crate::remote::radix36(now_ms() as u64)), name: x.name.clone(), kind: "ssh".into(), os: "linux".into(), host: x.host.clone(), ts_host: x.host.clone(), port: 22, user: x.user.clone(), color: "#76B900".into(), icon: x.icon.clone(), invited: true, ..Default::default() });
                }
                app.save_now();
                for m in app.cfg.machines.clone() { if m.is_ssh() && !m.ts_host.is_empty() && app.status_of(&m) != Status::Online { app.connect_machine(&m.id, false); } }
                app.toast_ok(format!("Added {} machine{}. Connecting over Tailscale…", fresh.len(), if fresh.len() > 1 { "s" } else { "" }));
                app.close_share();
            });
        });
    }

    pub(super) fn draw_share(&mut self) {
        let Some(mut sh) = self.share.take() else { return };
        enum Do { None, Close, Refresh, Install, Join(String), SaveToken, ClearToken, Mode(bool), Plan, CancelPlan, Apply, Invite, Remove(String), CopyMsg, CopyCode, JoinCode }
        let mut todo = Do::None;
        let esc = self.dialogs.is_empty() && self.ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let screen = self.ctx.content_rect();
        let per_guest = self.cfg.tailscale.as_ref().and_then(|t| t.per_guest_users).unwrap_or_else(|| !sh.overview.as_ref().is_some_and(|o| !o.guests.is_empty()));
        let users = self.ssh_users().join(" / ");
        let step = |ui: &mut egui::Ui, n: u8, title: &str, done: bool, add: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::new().fill(PANEL2).corner_radius(10).inner_margin(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 11.0, if done { tint(OK, 0.3) } else { PANEL3 });
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, if done { "✓".to_string() } else { n.to_string() }, FontId::proportional(11.5), if done { OK } else { TEXT2 });
                    ui.label(RichText::new(title).strong());
                });
                add(ui);
            });
            ui.add_space(6.0);
        };
        egui::Modal::new(egui::Id::unique("share")).frame(super::overlays::modal_frame()).show(&self.ctx, |ui| {
            ui.set_width(680.0f32.min(screen.width() - 60.0));
            ui.horizontal(|ui| { ui.label(RichText::new("Share your machines").size(16.0).family(medium())); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close").clicked() { todo = Do::Close; } }); });
            egui::ScrollArea::vertical().max_height(screen.height() - 150.0).min_scrolled_height((screen.height() - 150.0).max(120.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_text_btn(ui, "refresh", "Refresh", false).clicked() && !sh.busy { todo = Do::Refresh; }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| ui.add(egui::Label::new(dim("Invite people to your tailnet so they can use Claude, Antigravity and OpenClaw on your Linux machines from anywhere. They can reach those machines over SSH and nothing else.")).wrap()));
                    });
                });
                if !sh.error.is_empty() { egui::Frame::new().fill(tint(BAD, 0.12)).corner_radius(7).inner_margin(6).show(ui, |ui| ui.label(RichText::new(&sh.error).color(BAD))); }
                let Some(ts) = sh.ts.clone() else { ui.label(dim("Checking Tailscale…")); return };
                let local = &ts.local;
                let pc_done = local.installed && local.state == "Running";

                step(ui, 1, "This PC on Tailscale", pc_done, &mut |ui| {
                    if !local.installed { ui.label("Tailscale isn't installed on this PC."); if primary_btn(ui, "Install Tailscale").clicked() { todo = Do::Install; } }
                    else if !pc_done { ui.label(format!("Tailscale is installed but not signed in{}. Open the Tailscale app and sign in, then press Refresh.", if local.state.is_empty() { String::new() } else { format!(" ({})", local.state) })); }
                    else { ui.horizontal_wrapped(|ui| { ui.label(format!("Connected to {} as", local.tailnet.as_deref().unwrap_or("your tailnet"))); ui.label(RichText::new(local.self_peer.as_ref().map_or("", |p| p.dns_name.as_str())).monospace()); }); }
                });

                let machines = self.ssh_machines();
                let peers: Vec<Option<Peer>> = machines.iter().map(|m| if pc_done { self.peer_for(m, &ts) } else { None }).collect();
                step(ui, 2, "Machines on your tailnet", pc_done && !machines.is_empty() && peers.iter().all(Option::is_some), &mut |ui| {
                    for (m, p) in machines.iter().zip(&peers) {
                        ui.horizontal(|ui| {
                            logo_img(ui, self.machine_icon(m), 16.0);
                            ui.label(RichText::new(&m.name).strong());
                            ui.label(dim(&m.user));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match p {
                                Some(p) => { ui.label(RichText::new(&p.dns_name).monospace().size(11.0).color(TEXT3)); chip(ui, if p.online { "online" } else { "offline" }, if p.online { OK } else { TEXT3 }); }
                                None if self.status_of(m) == Status::Online => { if ui.button("Join Tailscale").clicked() { todo = Do::Join(m.id.clone()); } }
                                None => { ui.label(dim("connect to it on the LAN first")); }
                            });
                        });
                    }
                    ui.label(dim("\"Join Tailscale\" opens a terminal on that machine running sudo tailscale up --ssh: type the sudo password, then open the login link it prints.").size(11.0));
                });

                step(ui, 3, "Tailscale credential", ts.has_token, &mut |ui| {
                    if ts.has_token {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("{} saved in your {}.", if ts.token_kind == Some("oauth") { "OAuth client" } else { "API token" }, if IS_MAC { "Keychain" } else if cfg!(windows) { "Windows Credential Manager" } else { "keyring" }));
                            if ts.token_kind == Some("api") { ui.label(dim("Tip: an OAuth client scoped to Nexus is safer than a personal token and never expires.")); }
                            if ui.link("Remove").clicked() { todo = Do::ClearToken; }
                        });
                    } else {
                        ui.label("Nexus needs a Tailscale credential to send invites and set access rules.");
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Recommended: an OAuth client at");
                            if ui.link("login.tailscale.com → Settings → OAuth clients").clicked() { crate::platform::open_external("https://login.tailscale.com/admin/settings/oauth"); }
                            ui.label("with the scopes devices:core (tag tag:nexus-spark), policy_file and users; paste its secret (tskey-client-…). It only has the rights Nexus needs and never expires.");
                        });
                        ui.label(dim("The tag must exist first: add \"tag:nexus-spark\": [\"autogroup:admin\"] under tagOwners in your policy file, or run Review → Apply once with an API token.").size(11.0));
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Alternatively, an API access token (tskey-api-…, your full admin rights, expires within 90 days) from");
                            if ui.link("Settings → Keys").clicked() { crate::platform::open_external("https://login.tailscale.com/admin/settings/keys"); }
                        });
                        if !ts.can_encrypt { ui.label(RichText::new("No keychain is available on this computer (on Linux: gnome-keyring or KWallet), so a credential can't be saved.").color(WARN)); }
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut sh.token).password(true).hint_text("tskey-client-… or tskey-api-…").desired_width(380.0));
                            if ui.add_enabled(!sh.busy, |ui: &mut egui::Ui| primary_btn(ui, "Save")).clicked() { todo = Do::SaveToken; }
                        });
                    }
                });
                if !ts.has_token { return; }

                step(ui, 4, "Access rules", false, &mut |ui| {
                    if let Some(plan) = &sh.plan {
                        ui.label("Nexus will make these changes to your tailnet:");
                        if plan.changes.is_empty() { ui.label("• Nothing: already set up."); }
                        for c in &plan.changes { ui.label(format!("• {c}")); }
                        if !plan.missing.is_empty() { ui.label(RichText::new(format!("Not on the tailnet yet: {}. Join them first (step 2).", plan.missing.join(", "))).color(WARN)); }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!plan.changes.is_empty() && !sh.busy, |ui: &mut egui::Ui| primary_btn(ui, "Apply changes")).clicked() { todo = Do::Apply; }
                            if ui.button("Cancel").clicked() { todo = Do::CancelPlan; }
                        });
                        return;
                    }
                    let mut pg = per_guest;
                    if switch(ui, &mut pg, "Give each guest their own Linux account (nexus-<name>), created on the shared machines when you invite them (recommended)").changed() { todo = Do::Mode(pg); }
                    if per_guest {
                        ui.label(format!("Guests get SSH to these machines only, each as their own account, through Tailscale SSH: no keys to hand out, and removing a guest removes their login at once. Their files stay in /home/nexus-<name>. Your own access (as {users}) is unchanged."));
                        ui.label(RichText::new("Creating accounts needs sudo on each machine (Nexus opens a tab and asks). Guests still share the GPU, RAM and anything world-readable; agents they run sign in with their own credentials.").size(11.0).color(WARN));
                    } else {
                        ui.label(format!("Guests get SSH to these machines only, as {users}, through Tailscale SSH: no keys to hand out, and removing a guest cuts access straight away. Your own access stays unchanged."));
                        ui.label(RichText::new("Guests log in as these same accounts. They can read everything those accounts can: files, SSH keys, Claude / Antigravity / OpenClaw sign-ins, and your tmux sessions (read-only watching is a courtesy, not a lock). They could also leave something behind that keeps running after you remove them. Invite only people you'd trust with that account. Safer: tick the box above.").size(11.0).color(WARN));
                    }
                    let tagged = sh.overview.as_ref().is_some_and(|o| !o.sparks.is_empty() && o.sparks.iter().all(|s| s.device.as_ref().is_some_and(|d| d.tags.iter().any(|t| t == crate::tailscale::SPARK_TAG))));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!sh.busy, egui::Button::new(if tagged { "Re-check access rules" } else { "Review access rules…" })).clicked() { todo = Do::Plan; }
                        if tagged { chip(ui, "Machines tagged and locked down", OK); }
                    });
                });

                step(ui, 5, "Guests", false, &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut sh.email).hint_text("friend@example.com").desired_width(300.0));
                        if ui.add_enabled(!sh.busy, |ui: &mut egui::Ui| primary_btn(ui, "Invite")).clicked() { todo = Do::Invite; }
                    });
                    if let Some((email, message, _)) = &sh.result {
                        ui.label(RichText::new(format!("Invite ready for {email}. Tailscale has emailed them too. Send them this message:")).strong());
                        let mut text = message.clone();
                        ui.add(egui::TextEdit::multiline(&mut text).desired_rows(8).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                        ui.horizontal(|ui| { if primary_btn(ui, "Copy message").clicked() { todo = Do::CopyMsg; } if ui.button("Copy invite code only").clicked() { todo = Do::CopyCode; } });
                    }
                    match sh.overview.as_ref().map(|o| o.guests.as_slice()) {
                        Some(list) if !list.is_empty() => for g in list {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&g.email).strong());
                                chip(ui, &g.status, if g.status == "active" { OK } else { WARN });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.add_enabled(!sh.busy, egui::Button::new(RichText::new("Remove").color(BAD)).frame(false)).clicked() { todo = Do::Remove(g.email.clone()); } });
                            });
                        },
                        _ => { ui.label(dim("No guests yet.")); }
                    }
                });
            });
            ui.separator();
            ui.label(RichText::new("Got an invite code?").strong());
            ui.label(dim("Sign in to Tailscale with the invite link first, then paste the code to add the shared machines."));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut sh.code).hint_text("nexus-invite:…").desired_width(440.0));
                if ui.button("Add machines").clicked() { todo = Do::JoinCode; }
            });
        });
        self.share = Some(sh);
        if esc && matches!(todo, Do::None) { todo = Do::Close; }
        let busy = |app: &mut App| { if let Some(s) = app.share.as_mut() { s.busy = true; } };
        match todo {
            Do::None => {}
            Do::Close => self.close_share(),
            Do::Refresh => { busy(self); self.refresh_share(); }
            Do::Install => {
                if IS_MAC { crate::platform::open_external("https://tailscale.com/download/mac"); return; }
                self.close_share();
                self.launch("local", "shell", Extra { cmd: Some(if cfg!(windows) { "winget install -e --id Tailscale.Tailscale" } else { "curl -fsSL https://tailscale.com/install.sh | sh && sudo tailscale up" }.into()), name: Some("Install Tailscale".into()), ..Default::default() });
            }
            Do::Join(id) => {
                let name = self.machine(&id).name;
                self.close_share();
                self.launch(&id, "shell", Extra { raw: true, cmd: Some("sudo tailscale up --ssh && tailscale status | head -5".into()), name: Some(format!("Tailscale → {name}")), ..Default::default() });
            }
            Do::SaveToken => {
                busy(self);
                let (be, token) = (self.be.clone(), self.share.as_mut().map(|s| std::mem::take(&mut s.token)).unwrap_or_default());
                self.be.run(async move { be.ts_set_token(token).await }, |app, r| match r {
                    Ok(n) => { app.toast_ok(format!("Credential saved. {n} devices on your tailnet.")); app.refresh_share(); }
                    Err(e) => app.share_err(e),
                });
            }
            Do::ClearToken => { let be = self.be.clone(); self.be.run(async move { be.ts_clear_token().await }, |app, ()| { if let Some(s) = app.share.as_mut() { s.overview = None; } app.refresh_share(); }); }
            Do::Mode(on) => {
                self.cfg.tailscale.get_or_insert_with(Default::default).per_guest_users = Some(on);
                self.save_now();
                if let Some(s) = self.share.as_mut() { s.plan = None; }
            }
            Do::Plan => {
                busy(self);
                let (be, args) = (self.be.clone(), self.plan_args(None, None));
                self.be.run(async move { be.ts_plan(args).await }, |app, r| match r { Ok(p) => { if let Some(s) = app.share.as_mut() { s.plan = Some(p); s.busy = false; } } Err(e) => app.share_err(e) });
            }
            Do::CancelPlan => { if let Some(s) = self.share.as_mut() { s.plan = None; } }
            Do::Apply => {
                busy(self);
                let (be, etag) = (self.be.clone(), self.share.as_ref().and_then(|s| s.plan.as_ref()).and_then(|p| p.etag.clone()));
                self.be.run(async move { be.ts_apply(etag).await }, |app, r| match r {
                    Ok(()) => { if let Some(s) = app.share.as_mut() { s.plan = None; } app.toast_ok("Access rules applied and machines tagged"); app.refresh_share(); }
                    Err(e) => app.share_err(e),
                });
            }
            Do::Invite => { let email = self.share.as_ref().map(|s| s.email.trim().to_lowercase()).unwrap_or_default(); self.invite(email); }
            Do::Remove(email) => self.remove_guest(email),
            Do::CopyMsg => { if let Some((_, m, _)) = self.share.as_ref().and_then(|s| s.result.clone()) { self.ctx.copy_text(m); self.toast_ok("Invitation copied"); } }
            Do::CopyCode => { if let Some((_, _, c)) = self.share.as_ref().and_then(|s| s.result.clone()) { self.ctx.copy_text(c); self.toast_ok("Invite code copied"); } }
            Do::JoinCode => { let code = self.share.as_ref().map(|s| s.code.clone()).unwrap_or_default(); self.join_with_code(code); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_account_commands_and_email_check() {
        let c = useradd_cmd("nexus-alice", "alice@x.com");
        assert!(c.starts_with("u=nexus-alice; ") && c.contains("-c 'Nexus guest alice@x.com' \"$u\"") && !c.contains("userdel"));
        assert!(userlock_cmd("nexus-alice").contains("usermod -L"));
        assert!(valid_email("a@b.co") && !valid_email("a@b") && !valid_email("a b@c.d") && !valid_email("@b.co") && !valid_email("a@b@c.d"));
    }
}
