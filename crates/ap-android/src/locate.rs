//! Find the `adb` binary: config, PATH, ANDROID_HOME / ANDROID_SDK_ROOT,
//! then common install folders.

use std::path::{Path, PathBuf};

use ap_core::{AndroidConfig, Error, Result};

/// Fix hint shown when adb is missing.
pub const INSTALL_HINT: &str = "install Google platform-tools (`winget install Google.PlatformTools` \
     on Windows, or download from https://developer.android.com/tools/releases/platform-tools), \
     then put it on PATH or set android.adb_path in the config";

#[cfg(windows)]
const ADB_EXE: &str = "adb.exe";
#[cfg(not(windows))]
const ADB_EXE: &str = "adb";

/// Locate adb. A configured `adb_path` must exist; otherwise the search list is tried.
pub fn locate_adb(config: &AndroidConfig) -> Result<PathBuf> {
    if let Some(p) = &config.adb_path {
        let p = if p.is_dir() {
            p.join(ADB_EXE)
        } else {
            p.clone()
        };
        return if p.is_file() {
            Ok(p)
        } else {
            Err(Error::setup(
                format!(
                    "android.adb_path points to {}, which does not exist",
                    p.display()
                ),
                format!("fix android.adb_path in the config, or remove it and {INSTALL_HINT}"),
            ))
        };
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    candidates(&env)
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| Error::setup("adb (Android platform-tools) was not found", INSTALL_HINT))
}

/// Ordered search list. `env` reads environment variables (injectable for tests).
pub fn candidates(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = env("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(ADB_EXE)));
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = env(var) {
            out.push(sdk.join("platform-tools").join(ADB_EXE));
        }
    }
    if cfg!(windows) {
        if let Some(local) = env("LOCALAPPDATA") {
            out.push(sdk_tools(&local.join("Android").join("Sdk")));
            out.push(
                local
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links")
                    .join(ADB_EXE),
            );
        }
        if let Some(home) = env("USERPROFILE") {
            out.push(home.join("platform-tools").join(ADB_EXE));
            out.push(sdk_tools(&home.join("AppData/Local/Android/Sdk")));
        }
        for root in ["C:\\", "D:\\"] {
            let r = Path::new(root);
            out.push(r.join("Android").join("platform-tools").join(ADB_EXE));
            out.push(sdk_tools(&r.join("Android").join("Sdk")));
            out.push(r.join("platform-tools").join(ADB_EXE));
        }
        if let Some(pf) = env("ProgramFiles") {
            out.push(pf.join("platform-tools").join(ADB_EXE));
        }
    } else if let Some(home) = env("HOME") {
        out.push(sdk_tools(&home.join("Android").join("Sdk")));
        out.push(sdk_tools(&home.join("Library").join("Android").join("sdk")));
        out.push(PathBuf::from("/usr/bin/adb"));
        out.push(PathBuf::from("/opt/homebrew/bin/adb"));
    }
    out
}

fn sdk_tools(sdk: &Path) -> PathBuf {
    sdk.join("platform-tools").join(ADB_EXE)
}

/// True when `host` is this machine, so we may start a server ourselves.
pub fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim_matches(['[', ']']),
        "127.0.0.1" | "localhost" | "::1" | ""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_order() {
        let env = |k: &str| match k {
            "ANDROID_HOME" => Some(PathBuf::from("/sdk")),
            "LOCALAPPDATA" => Some(PathBuf::from("C:/Users/x/AppData/Local")),
            _ => None,
        };
        let c = candidates(&env);
        assert_eq!(c[0], Path::new("/sdk").join("platform-tools").join(ADB_EXE));
        if cfg!(windows) {
            assert!(
                c.iter()
                    .any(|p| p.ends_with("Android/Sdk/platform-tools/adb.exe"))
            );
            assert!(
                c.iter()
                    .any(|p| p.starts_with("D:\\Android\\platform-tools"))
            );
        }
    }

    #[test]
    fn missing_configured_path_is_setup_error() {
        let cfg = AndroidConfig {
            adb_path: Some(PathBuf::from("Z:/nope/adb.exe")),
            ..AndroidConfig::default()
        };
        let e = locate_adb(&cfg).unwrap_err();
        assert!(
            e.hint()
                .unwrap()
                .contains("winget install Google.PlatformTools")
        );
    }

    #[test]
    fn loopback() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("192.168.1.2"));
    }
}
