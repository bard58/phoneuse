//! OCR model files: where they live, and downloading them with progress and
//! SHA-256 verification. Models are never bundled in the binary.
//!
//! The default models are the `ocrs` project's ONNX models (text detection
//! ~2.5 MB, text recognition ~9.5 MB), the same files its
//! `download-models.sh` fetches.
//!
//! Checksums: the default models have pinned SHA-256 hashes. A download must
//! match the pinned hash, and [`crate::OcrsRecognizer::load`] checks the files
//! against it before every load. A custom [`ModelFile`] without a pinned hash
//! falls back to trust on first use: the hash of the first download is
//! recorded next to the file (`<name>.sha256`) and later checks compare with
//! it. That sidecar sits in the same writable folder, so it protects against
//! corruption, not against a local attacker.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ap_core::{Error, Result};
use futures::StreamExt;
use sha2::{Digest, Sha256};

/// File name of the text detection model.
pub const DETECTION_MODEL: &str = "text-detection.onnx";
/// File name of the text recognition model.
pub const RECOGNITION_MODEL: &str = "text-recognition.onnx";

/// Pinned SHA-256 of [`DETECTION_MODEL`] (2_499_479 bytes).
pub const DETECTION_SHA256: &str =
    "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe";
/// Pinned SHA-256 of [`RECOGNITION_MODEL`] (9_713_177 bytes).
pub const RECOGNITION_SHA256: &str =
    "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5";

const BASE_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";

/// One model file to fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelFile {
    /// File name in the model directory.
    pub name: String,
    pub url: String,
    /// Expected SHA-256 (hex). `None`: trust on first use.
    pub sha256: Option<String>,
    /// Approximate size in bytes, for messages before the download starts.
    pub approx_size: u64,
}

/// The `ocrs` detection and recognition models.
///
/// The upstream project publishes no checksums. The pinned hashes come from
/// a download on 2026-11-08, cross-checked against a second S3 endpoint
/// (`ocrs-models.s3.amazonaws.com`). If upstream replaces a model, update
/// the hash here.
pub fn default_models() -> Vec<ModelFile> {
    vec![
        ModelFile {
            name: DETECTION_MODEL.into(),
            url: format!("{BASE_URL}/{DETECTION_MODEL}"),
            sha256: Some(DETECTION_SHA256.into()),
            approx_size: 2_500_000,
        },
        ModelFile {
            name: RECOGNITION_MODEL.into(),
            url: format!("{BASE_URL}/{RECOGNITION_MODEL}"),
            sha256: Some(RECOGNITION_SHA256.into()),
            approx_size: 9_700_000,
        },
    ]
}

/// `<platform cache dir>/agentphone/models/ocrs` (e.g.
/// `%LOCALAPPDATA%\agentphone\cache\models\ocrs` on Windows).
pub fn default_model_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "agentphone")
        .map(|d| d.cache_dir().join("models").join("ocrs"))
        .ok_or_else(|| Error::Config("cannot find the user cache directory".into()))
}

/// Download progress for one file.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    pub file: String,
    pub downloaded: u64,
    /// Total size when the server sent it.
    pub total: Option<u64>,
    /// The file was already present and verified; nothing was downloaded.
    pub cached: bool,
    /// This file is complete.
    pub done: bool,
}

/// Hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sidecar(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

/// Check a model file against its pinned hash, or else against the hash
/// recorded at download time. Returns the file's hash.
pub fn verify_model(dir: &Path, model: &ModelFile) -> Result<String> {
    let path = dir.join(&model.name);
    let actual = sha256_file(&path)?;
    let expected = match &model.sha256 {
        Some(e) => Some(e.trim().to_lowercase()),
        None => fs::read_to_string(sidecar(&path))
            .ok()
            .map(|s| s.trim().to_lowercase()),
    };
    match expected {
        Some(e) if e != actual => Err(Error::Protocol(format!(
            "checksum mismatch for {}: expected {e}, got {actual}",
            path.display()
        ))),
        Some(_) => Ok(actual),
        None => Err(Error::Protocol(format!(
            "no checksum recorded for {}",
            path.display()
        ))),
    }
}

/// Check every model in `dir` before loading it. Any missing file, checksum
/// mismatch or missing recorded checksum gives an [`Error::Setup`] that
/// names the problem and tells the user to download the models again.
pub fn check_models(dir: &Path, models: &[ModelFile]) -> Result<()> {
    let hint = "run `agentphone setup ocr` to download the models again (about 12 MB)";
    if let Some(m) = models.iter().find(|m| !dir.join(&m.name).is_file()) {
        return Err(Error::setup(
            format!("OCR model {} not found in {}", m.name, dir.display()),
            hint,
        ));
    }
    for m in models {
        verify_model(dir, m).map_err(|e| {
            Error::setup(
                format!("OCR model {} failed verification: {e}", m.name),
                hint,
            )
        })?;
    }
    Ok(())
}

/// True when every model is present and verifies.
pub fn models_ready(dir: &Path, models: &[ModelFile]) -> bool {
    models.iter().all(|m| verify_model(dir, m).is_ok())
}

/// Download `models` into `dir` (created if needed). Files that are present
/// and verify are skipped. Each file is written to `<name>.part`, hashed
/// while streaming, checked against `sha256` when pinned, then renamed into
/// place. `progress` is called as bytes arrive.
pub async fn download_models(
    dir: &Path,
    models: &[ModelFile],
    mut progress: impl FnMut(&DownloadProgress),
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    // reqwest is built with `rustls-no-provider`; use ring (already in the tree).
    let _ = rustls::crypto::ring::default_provider().install_default();
    // No overall timeout (a slow link may need minutes for 12 MB), but a
    // stalled connect or read fails instead of hanging.
    let client = reqwest::Client::builder()
        .user_agent(concat!("agentphone/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Other(format!("HTTP client: {e}")))?;
    let mut paths = Vec::with_capacity(models.len());
    for m in models {
        let path = dir.join(&m.name);
        if path.is_file() && verify_model(dir, m).is_ok() {
            let size = fs::metadata(&path).map(|md| md.len()).unwrap_or(0);
            progress(&DownloadProgress {
                file: m.name.clone(),
                downloaded: size,
                total: Some(size),
                cached: true,
                done: true,
            });
            paths.push(path);
            continue;
        }
        tracing::info!(url = %m.url, "downloading OCR model");
        let net = |e: reqwest::Error| {
            Error::setup(
                format!("cannot download {}: {e}", m.url),
                "check the internet connection (or proxy) and run `agentphone setup ocr` again",
            )
        };
        let resp = client
            .get(&m.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(net)?;
        let total = resp.content_length();
        let mut part = path.as_os_str().to_owned();
        part.push(".part");
        let part = PathBuf::from(part);
        let mut file = fs::File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut downloaded = 0u64;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = fs::remove_file(&part);
                    return Err(net(e));
                }
            };
            file.write_all(&chunk)?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;
            progress(&DownloadProgress {
                file: m.name.clone(),
                downloaded,
                total,
                cached: false,
                done: false,
            });
        }
        file.flush()?;
        drop(file);
        let actual = hex(&hasher.finalize());
        if let Some(expected) = &m.sha256
            && expected.trim().to_lowercase() != actual
        {
            let _ = fs::remove_file(&part);
            return Err(Error::Protocol(format!(
                "checksum mismatch for {}: expected {expected}, got {actual}; the file was deleted",
                m.url
            )));
        }
        fs::rename(&part, &path)?;
        fs::write(sidecar(&path), format!("{actual}\n"))?;
        progress(&DownloadProgress {
            file: m.name.clone(),
            downloaded,
            total,
            cached: false,
            done: true,
        });
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ap-vision-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn verify_pinned_and_recorded() {
        let d = tmpdir("verify");
        fs::write(d.join("m.onnx"), b"abc").unwrap();
        // sha256("abc")
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&d.join("m.onnx")).unwrap(), abc);
        let mut m = ModelFile {
            name: "m.onnx".into(),
            url: "https://example.invalid/m.onnx".into(),
            sha256: Some(abc.to_uppercase()),
            approx_size: 3,
        };
        assert!(verify_model(&d, &m).is_ok());
        m.sha256 = Some("00".into());
        assert!(verify_model(&d, &m).is_err());
        // Trust on first use: no pin, no sidecar -> not ready; sidecar -> ok.
        m.sha256 = None;
        assert!(!models_ready(&d, std::slice::from_ref(&m)));
        fs::write(sidecar(&d.join("m.onnx")), format!("{abc}\n")).unwrap();
        assert!(models_ready(&d, std::slice::from_ref(&m)));
        fs::write(d.join("m.onnx"), b"tampered").unwrap();
        assert!(verify_model(&d, &m).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn cached_models_are_not_downloaded() {
        let d = tmpdir("cached");
        fs::write(d.join("m.onnx"), b"abc").unwrap();
        let m = ModelFile {
            name: "m.onnx".into(),
            // Never contacted: the file is present and verifies.
            url: "https://example.invalid/m.onnx".into(),
            sha256: Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()),
            approx_size: 3,
        };
        let mut events = Vec::new();
        let paths = download_models(&d, &[m], |p| events.push(p.clone()))
            .await
            .unwrap();
        assert_eq!(paths, vec![d.join("m.onnx")]);
        assert!(events.len() == 1 && events[0].cached && events[0].done);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn check_models_rejects_missing_and_tampered() {
        let d = tmpdir("check");
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let m = ModelFile {
            name: "m.onnx".into(),
            url: "https://example.invalid/m.onnx".into(),
            sha256: Some(abc.into()),
            approx_size: 3,
        };
        let e = check_models(&d, std::slice::from_ref(&m)).unwrap_err();
        assert!(
            e.hint().is_some() && e.to_string().contains("not found"),
            "{e}"
        );
        fs::write(d.join("m.onnx"), b"tampered").unwrap();
        let e = check_models(&d, std::slice::from_ref(&m)).unwrap_err();
        assert!(
            e.hint().is_some() && e.to_string().contains("checksum mismatch"),
            "{e}"
        );
        fs::write(d.join("m.onnx"), b"abc").unwrap();
        check_models(&d, &[m]).unwrap();
        // The default models in an empty folder: a setup error, not a panic.
        let e = check_models(&d, &default_models()).unwrap_err();
        assert!(e.hint().unwrap().contains("agentphone setup ocr"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn defaults() {
        let m = default_models();
        assert_eq!(m.len(), 2);
        assert!(m[0].url.ends_with(DETECTION_MODEL));
        assert!(m[1].url.ends_with(RECOGNITION_MODEL));
        // Both default models are pinned (64 lowercase hex chars).
        for f in &m {
            let h = f.sha256.as_deref().expect("pinned");
            assert_eq!(h.len(), 64);
            assert!(
                h.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
        assert!(default_model_dir().unwrap().ends_with("ocrs"));
    }
}
