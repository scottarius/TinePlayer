//! Embeds the application icon into the Windows executable.
//!
//! Windows takes an executable's icon from a resource compiled into the
//! binary, not from a file beside it, so this is the only way Explorer, the
//! taskbar and Alt-Tab get anything other than a blank default. Linux has no
//! equivalent: there the desktop entry names the icon, and
//! `integrations/install-desktop-linux.sh` puts both where the desktop can
//! find them.

fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=data/branding/tineplayer.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("data/branding/tineplayer.ico");
        if let Err(e) = resource.compile() {
            // Not fatal: a binary without an icon still runs, and failing the
            // build over decoration would be worse than the missing icon.
            println!("cargo:warning=Could not embed the icon: {e}");
        }
    }
}
