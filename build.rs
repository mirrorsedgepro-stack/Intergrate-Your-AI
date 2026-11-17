// Windows: give the executables the Nexus icon and version details. Nothing to do elsewhere.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Nexus");
        res.set("FileDescription", "Nexus");
        res.set("CompanyName", "mirrorsedgepro-stack");
        if let Err(e) = res.compile() { println!("cargo:warning=could not embed the Windows icon: {e}"); }
    }
}
