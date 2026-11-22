//! Android: the entry point, the screen area that system bars and the on-screen keyboard leave free, and the two
//! system services Nexus needs that have no native API (the clipboard, and opening a link), reached through JNI.
//!
//! The app is a NativeActivity: one shared library and no Java code of its own.
use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::{jni_sig, jni_str};
use std::sync::Mutex;

static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);

#[no_mangle]
fn android_main(app: AndroidApp) {
    // The app's private storage is home: settings, SSH keys and pinned host keys live there, where no other app
    // can read them. Temporary files (pasted images, uploads in flight) go to the app's cache folder.
    let files = app.internal_data_path().unwrap_or_else(|| std::path::PathBuf::from("/data/local/tmp"));
    let cache = files.parent().map_or_else(|| files.join("cache"), |p| p.join("cache"));
    let _ = std::fs::create_dir_all(&files);
    let _ = std::fs::create_dir_all(&cache);
    // Before any thread of ours exists: this changes the process environment.
    std::env::set_var("HOME", &files);
    std::env::set_var("TMPDIR", &cache);
    if std::env::var_os("SHELL").is_none() { std::env::set_var("SHELL", "/system/bin/sh"); }
    *APP.lock().unwrap() = Some(app.clone());

    if crate::single_instance() {
        let (cfg, rt) = crate::prepare();
        let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
        let _ = eframe::run_native("Nexus", options, Box::new(move |cc| Ok(Box::new(crate::ui::App::new(cc, rt, cfg)))));
    }
    // The activity is gone. An event loop can only be made once per process, so end the process: opening Nexus
    // again starts a fresh one (remote sessions keep running in tmux and are reattached).
    std::process::exit(0);
}

/// How much of each window edge is covered by system bars, a display cutout or the on-screen keyboard, in physical
/// pixels: (left, top, right, bottom).
pub fn insets() -> (f32, f32, f32, f32) {
    let none = (0.0, 0.0, 0.0, 0.0);
    let Some(app) = APP.lock().unwrap().clone() else { return none };
    let Some(win) = app.native_window() else { return none };
    let (w, h) = (win.width(), win.height());
    let r = app.content_rect();
    // Before the first layout the rectangle is empty; a rectangle outside the window is one from before a rotation.
    if w <= 0 || h <= 0 || r.right <= r.left || r.bottom <= r.top || r.right > w || r.bottom > h { return none; }
    let (l, t, rt, b) = (r.left.max(0), r.top.max(0), (w - r.right).max(0), (h - r.bottom).max(0));
    // Never give up more than most of the window: the terminal has to stay usable whatever the system reports.
    if l + rt > w * 3 / 4 || t + b > h * 3 / 4 { return none; }
    (l as f32, t as f32, rt as f32, b as f32)
}

/// Run something against the Java side. Any Java exception is caught and reported as a failure.
fn with_activity<T>(f: impl FnOnce(&mut jni::Env, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let ctx = ndk_context::android_context();
    if ctx.vm().is_null() || ctx.context().is_null() { return None; }
    // Safety: android-activity registered these pointers, and keeps the activity reference alive while it runs us.
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<T> {
        let activity = unsafe { JObject::from_raw(env, ctx.context().cast()) };
        f(env, &activity)
    }).ok()
}

fn clipboard<'a>(env: &mut jni::Env<'a>, activity: &JObject) -> jni::errors::Result<JObject<'a>> {
    let name = env.new_string("clipboard")?;
    env.call_method(activity, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&name)])?.l()
}

/// Put text on the system clipboard.
pub fn set_clipboard(text: &str) -> bool {
    with_activity(|env, activity| {
        let cm = clipboard(env, activity)?;
        if cm.is_null() { return Ok(false); }
        let (label, body) = (env.new_string("Nexus")?, env.new_string(text)?);
        let clip = env.call_static_method(jni_str!("android/content/ClipData"), jni_str!("newPlainText"),
            jni_sig!("(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"), &[JValue::Object(&label), JValue::Object(&body)])?.l()?;
        env.call_method(&cm, jni_str!("setPrimaryClip"), jni_sig!("(Landroid/content/ClipData;)V"), &[JValue::Object(&clip)])?;
        Ok(true)
    }).unwrap_or(false)
}

/// The text on the system clipboard (Android only hands it to the app in front).
pub fn get_clipboard() -> Option<String> {
    with_activity(|env, activity| {
        let cm = clipboard(env, activity)?;
        if cm.is_null() { return Ok(None); }
        let clip = env.call_method(&cm, jni_str!("getPrimaryClip"), jni_sig!("()Landroid/content/ClipData;"), &[])?.l()?;
        if clip.is_null() { return Ok(None); }
        if env.call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?.i()? < 1 { return Ok(None); }
        let item = env.call_method(&clip, jni_str!("getItemAt"), jni_sig!("(I)Landroid/content/ClipData$Item;"), &[JValue::Int(0)])?.l()?;
        if item.is_null() { return Ok(None); }
        let chars = env.call_method(&item, jni_str!("coerceToText"), jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"), &[JValue::Object(activity)])?.l()?;
        if chars.is_null() { return Ok(None); }
        let text = env.call_method(&chars, jni_str!("toString"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
        if text.is_null() { return Ok(None); }
        let text = env.cast_local::<JString>(text)?;
        Ok(Some(text.try_to_string(env)?).filter(|t| !t.is_empty()))
    }).flatten()
}

/// Open a web link in the browser. The caller has already checked that it is one.
pub fn open_url(url: &str) -> bool {
    with_activity(|env, activity| {
        let (action, url) = (env.new_string("android.intent.action.VIEW")?, env.new_string(url)?);
        let uri = env.call_static_method(jni_str!("android/net/Uri"), jni_str!("parse"), jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"), &[JValue::Object(&url)])?.l()?;
        let intent = env.new_object(jni_str!("android/content/Intent"), jni_sig!("(Ljava/lang/String;Landroid/net/Uri;)V"), &[JValue::Object(&action), JValue::Object(&uri)])?;
        env.call_method(&intent, jni_str!("addFlags"), jni_sig!("(I)Landroid/content/Intent;"), &[JValue::Int(0x1000_0000)])?; // FLAG_ACTIVITY_NEW_TASK
        env.call_method(activity, jni_str!("startActivity"), jni_sig!("(Landroid/content/Intent;)V"), &[JValue::Object(&intent)])?;
        Ok(true)
    }).unwrap_or(false)
}
