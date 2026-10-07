//! The llama.cpp runtime bundled with the app: detected and versioned, never downloaded.

use std::path::PathBuf;

use nlmx_application::ports::{BoxFuture, InferenceRuntime};
use nlmx_domain::models::RuntimeInfo;

pub struct LlamaCppRuntime {
    binary: Option<PathBuf>,
}

impl LlamaCppRuntime {
    /// Uses [`crate::llama_server_path`].
    pub fn detect() -> Self {
        Self {
            binary: crate::llama_server_path(),
        }
    }

    pub fn at(binary: PathBuf) -> Self {
        Self {
            binary: Some(binary),
        }
    }

    pub fn binary(&self) -> Option<&PathBuf> {
        self.binary.as_ref()
    }
}

/// Parses `version: 0.5.0-dev (build 11349, commit fb4b2737a)`.
pub fn parse_version(output: &str) -> Option<(String, Option<String>)> {
    let line = output
        .lines()
        .find(|l| l.trim_start().starts_with("version:"))?;
    let inside = line.split_once('(')?.1.split_once(')')?.0;
    let mut build = None;
    let mut commit = None;
    for part in inside.split(',').map(str::trim) {
        if let Some(n) = part.strip_prefix("build ") {
            build = Some(format!("b{n}"));
        } else if let Some(c) = part.strip_prefix("commit ") {
            commit = Some(c.to_string());
        }
    }
    Some((build?, commit))
}

impl InferenceRuntime for LlamaCppRuntime {
    fn info(&self) -> BoxFuture<'_, Result<RuntimeInfo, String>> {
        Box::pin(async move {
            let binary = self
                .binary
                .clone()
                .ok_or("runtime llama.cpp não encontrado (llama-server)")?;
            let output = tokio::process::Command::new(&binary)
                .arg("--version")
                .output()
                .await
                .map_err(|err| format!("não foi possível executar {}: {err}", binary.display()))?;
            // llama-server prints its version on stderr.
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let (build, commit) = parse_version(&text)
                .ok_or_else(|| format!("versão não reconhecida: {}", text.trim()))?;
            let path = binary.display().to_string();
            Ok(RuntimeInfo {
                name: "llama.cpp".into(),
                build,
                commit,
                bundled: path.contains(".app/Contents/"),
                path,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_line() {
        let out = "ggml_metal_device_init: ...\nversion: 0.5.0-dev (build 11349, commit fb4b2737a)\nbuilt with AppleClang";
        assert_eq!(
            parse_version(out),
            Some(("b11349".into(), Some("fb4b2737a".into())))
        );
        assert_eq!(parse_version("nothing here"), None);
    }

    #[tokio::test]
    async fn reads_the_real_runtime_when_present() {
        let Some(binary) = crate::llama_server_path() else {
            return;
        };
        let info = LlamaCppRuntime::at(binary).info().await.unwrap();
        assert_eq!(info.name, "llama.cpp");
        assert_eq!(
            info.build,
            crate::LLAMA_BUILD,
            "runtime/llama matches the pinned build"
        );
        assert!(!info.bundled);
    }

    #[tokio::test]
    async fn a_missing_runtime_is_reported() {
        let err = LlamaCppRuntime { binary: None }.info().await.unwrap_err();
        assert!(err.contains("não encontrado"));
    }
}
