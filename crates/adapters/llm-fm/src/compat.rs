//! System compatibility: Apple Foundation Models needs macOS 27+ on Apple Silicon, running
//! natively (not under Rosetta), with the system `fm` tool present.

use std::{os::unix::fs::PermissionsExt, path::Path};

pub const MIN_MACOS: u32 = 27;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// e.g. "27.0.1"; `None` if it could not be read.
    pub os_version: Option<String>,
    /// `std::env::consts::ARCH` of this binary.
    pub arch: String,
    /// Running under Rosetta translation.
    pub translated: bool,
}

impl SystemInfo {
    pub fn current() -> Self {
        Self {
            os_version: sysctl("kern.osproductversion"),
            arch: std::env::consts::ARCH.to_string(),
            translated: sysctl("sysctl.proc_translated").as_deref() == Some("1"),
        }
    }
}

fn sysctl(name: &str) -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !value.is_empty()).then_some(value)
}

/// Major version of "27.0.1" → 27.
pub fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse().ok()
}

/// `Err(reason)` (suitable for display) when this system can't run the model.
pub fn check(info: &SystemInfo, binary: &Path) -> Result<(), String> {
    match info.os_version.as_deref().and_then(major) {
        Some(v) if v >= MIN_MACOS => {}
        Some(_) | None => {
            let found = info.os_version.as_deref().unwrap_or("desconhecida");
            return Err(format!(
                "O Apple Foundation Models requer macOS {MIN_MACOS} ou superior (versão atual: {found})."
            ));
        }
    }
    if info.arch != "aarch64" {
        return Err("O Apple Foundation Models requer um Mac com Apple Silicon.".into());
    }
    if info.translated {
        return Err("O app está rodando pelo Rosetta; abra a versão para Apple Silicon.".into());
    }
    let executable = std::fs::metadata(binary)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);
    if !executable {
        return Err(format!(
            "Ferramenta do Apple Foundation Models não encontrada ({}).",
            binary.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(version: Option<&str>, arch: &str, translated: bool) -> SystemInfo {
        SystemInfo {
            os_version: version.map(String::from),
            arch: arch.into(),
            translated,
        }
    }

    #[test]
    fn requires_macos_27_apple_silicon_native_and_the_binary() {
        let sh = Path::new("/bin/sh");
        assert!(check(&info(Some("27.0.1"), "aarch64", false), sh).is_ok());
        assert!(check(&info(Some("28.0"), "aarch64", false), sh).is_ok());
        let old = check(&info(Some("26.4"), "aarch64", false), sh).unwrap_err();
        assert!(old.contains("macOS 27") && old.contains("26.4"), "{old}");
        assert!(check(&info(None, "aarch64", false), sh).is_err());
        assert!(
            check(&info(Some("27.0"), "x86_64", false), sh)
                .unwrap_err()
                .contains("Apple Silicon")
        );
        assert!(
            check(&info(Some("27.0"), "aarch64", true), sh)
                .unwrap_err()
                .contains("Rosetta")
        );
        assert!(
            check(
                &info(Some("27.0"), "aarch64", false),
                Path::new("/nonexistent/fm")
            )
            .is_err()
        );
        assert!(
            check(
                &info(Some("27.0"), "aarch64", false),
                Path::new("/etc/hosts")
            )
            .is_err(),
            "not executable"
        );
    }

    #[test]
    fn parses_major_versions() {
        assert_eq!(major("27.0.1"), Some(27));
        assert_eq!(major("28"), Some(28));
        assert_eq!(major("beta"), None);
    }

    #[test]
    fn this_machine_reports_its_system() {
        let info = SystemInfo::current();
        assert!(info.os_version.is_some());
        assert_eq!(info.arch, std::env::consts::ARCH);
    }
}
