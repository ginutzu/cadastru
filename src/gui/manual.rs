//! Manualul de utilizare (PDF) inclus în executabil, deschis din meniul Ajutor (F1).

use std::path::PathBuf;
use cadastru::i18n::tr;

/// Manualul, inclus la compilare din `docs/Manual_Cadastru.pdf`.
static MANUAL: &[u8] = include_bytes!("../../docs/Manual_Cadastru.pdf");

/// Unde se scrie manualul ca să poată fi deschis de programul implicit pentru PDF.
fn manual_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    Some(
        base.join("Cadastru")
            .join(format!("Manual_Cadastru_{}.pdf", env!("CARGO_PKG_VERSION"))),
    )
}

/// Scrie manualul (dacă lipsește sau diferă) și îl deschide. Întoarce calea.
pub fn open_manual() -> anyhow::Result<PathBuf> {
    let path = manual_path().ok_or_else(|| anyhow::anyhow!(tr!("nu găsesc un dosar pentru manual")))?;
    let same = std::fs::metadata(&path)
        .map(|m| m.len() == MANUAL.len() as u64)
        .unwrap_or(false);
    if !same {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        // dacă manualul e deja deschis (fișier blocat), îl folosim pe cel existent
        if let Err(e) = std::fs::write(&path, MANUAL) {
            if !path.exists() {
                return Err(e.into());
            }
        }
    }
    open_with_default_app(&path)?;
    Ok(path)
}

#[cfg(windows)]
fn open_with_default_app(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let file = wide(path.as_os_str());
    let verb = wide(std::ffi::OsStr::new("open"));
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // conform documentației, valorile <= 32 sunt coduri de eroare
    if r as isize <= 32 {
        anyhow::bail!(
            "{}", tr!("nu există un program asociat fișierelor PDF (cod {}). Manualul este în {}",
            r as isize,
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_with_default_app(path: &std::path::Path) -> anyhow::Result<()> {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn manual_is_pdf() {
        assert!(super::MANUAL.starts_with(b"%PDF-"));
        assert!(super::MANUAL.len() > 10_000);
    }
}
