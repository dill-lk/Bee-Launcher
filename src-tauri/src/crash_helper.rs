use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_TAIL_BYTES: u64 = 24 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CrashDiagnosis {
    pub found: bool,
    pub title: String,
    pub summary: String,
    pub likely_causes: Vec<String>,
    pub next_steps: Vec<String>,
    pub log_excerpt: String,
}

/// Read and analyze only the bounded tail of a crash log. No network or game files are accessed.
pub(super) fn analyze_log(log_path: &Path) -> CrashDiagnosis {
    let Some(log) = read_tail(log_path) else {
        return CrashDiagnosis {
            found: false,
            title: "Crash log not found".to_string(),
            summary: "Blockline could not find or read the crash log at the provided path."
                .to_string(),
            likely_causes: vec![
                "The crash log may not have been written yet, or the supplied path may be outdated."
                    .to_string(),
            ],
            next_steps: vec![
                "Try launching once more and then select the newest crash log.".to_string(),
                "If it still fails, provide the log path, your Java version, and the launcher/game version."
                    .to_string(),
            ],
            log_excerpt: String::new(),
        };
    };

    let excerpt = redact_excerpt(&log);
    let lower = log.to_ascii_lowercase();
    let mut causes = Vec::new();
    let mut steps = Vec::new();
    let mut title = String::new();
    let mut summary = String::new();

    // Keep these checks independent so a log can explain more than one contributing symptom.
    if contains_any(
        &lower,
        &[
            "outofmemoryerror",
            "java heap space",
            "gc overhead limit exceeded",
            "unable to create new native thread",
        ],
    ) {
        title = "Java ran out of memory".to_string();
        summary = "The Java process exhausted available heap or native memory while starting or running the game.".to_string();
        causes.push(
            "The allocated Java memory is too low for the game, mods, or current workload."
                .to_string(),
        );
        causes.push(
            "Other applications or a large mod/resource pack may be consuming available memory."
                .to_string(),
        );
        steps.push("Close other memory-heavy applications and retry.".to_string());
        steps.push(
            "Check the launcher's Java memory allocation and use a 64-bit Java runtime."
                .to_string(),
        );
        steps.push(
            "If mods are installed, retry with them disabled or remove recently added content."
                .to_string(),
        );
    }

    if contains_any(
        &lower,
        &[
            "opengl",
            "glfw",
            "context creation",
            "gpu",
            "graphics driver",
            "driver version",
            "failed to create display",
        ],
    ) {
        if title.is_empty() {
            title = "Graphics initialization failed".to_string();
            summary =
                "The game could not initialize its OpenGL window or graphics context.".to_string();
        }
        causes.push("The graphics driver, GPU selection, or OpenGL context may be incompatible or unavailable.".to_string());
        steps.push("Update the graphics driver from the GPU manufacturer's official support page and restart.".to_string());
        steps.push(
            "If the computer has multiple GPUs, try selecting the dedicated GPU for the launcher."
                .to_string(),
        );
        steps.push("Retry with overlays and graphics-heavy options disabled.".to_string());
    }

    if contains_any(
        &lower,
        &[
            "unsupportedclassversionerror",
            "class file version",
            "has been compiled by a more recent version",
            "class version mismatch",
        ],
    ) {
        if title.is_empty() {
            title = "Java version mismatch".to_string();
            summary = "The game or one of its components was built for a different Java version than the one being used.".to_string();
        }
        causes.push("The selected Java runtime is older or otherwise incompatible with the required class-file version.".to_string());
        steps.push(
            "Check the game's required Java version and select that version in the launcher."
                .to_string(),
        );
        steps.push(
            "Verify that the launcher is using the intended 64-bit Java executable, then retry."
                .to_string(),
        );
    }

    if contains_any(
        &lower,
        &[
            "mod resolution",
            "mixin",
            "injection failure",
            "mod loading",
            "modloading",
            "mod conflict",
            "no such method error",
        ],
    ) {
        if title.is_empty() {
            title = "Mod or mixin conflict detected".to_string();
            summary = "A mod, mixin, or injected component appears to be incompatible with the current game setup.".to_string();
        }
        causes
            .push("A mod may target a different game, loader, or dependency version.".to_string());
        steps.push(
            "Retry without recently added or updated mods, then add them back one at a time."
                .to_string(),
        );
        steps.push(
            "Confirm that each mod and dependency matches the exact game and loader versions."
                .to_string(),
        );
    }

    if contains_any(
        &lower,
        &[
            "classnotfoundexception",
            "noclassdeffounderror",
            "filenotfoundexception",
            "nosuchfile",
            "could not find",
            "library not found",
            "dll not found",
            "unable to load library",
        ],
    ) {
        if title.is_empty() {
            title = "A required class or file is missing".to_string();
            summary = "The game could not locate a required Java class, native library, or file."
                .to_string();
        }
        causes.push(
            "An installation, dependency, or mod file may be missing or incomplete.".to_string(),
        );
        steps.push(
            "Use the launcher's repair or verify-files option, if available, and retry."
                .to_string(),
        );
        steps.push("Check that required dependencies match the game and loader versions; avoid deleting user data until it is backed up.".to_string());
    }

    if contains_any(
        &lower,
        &[
            "java runtime",
            "jre",
            "java version",
            "could not find java",
            "java executable",
            "jvm",
            "java.lang.unsatisfiedlinkerror",
        ],
    ) && !contains_any(
        &lower,
        &[
            "unsupportedclassversionerror",
            "class file version",
            "has been compiled by a more recent version",
            "class version mismatch",
        ],
    ) {
        if title.is_empty() {
            title = "Java runtime problem".to_string();
            summary =
                "The launcher reported a problem starting or using the Java runtime.".to_string();
        }
        causes.push(
            "Java may be missing, misconfigured, or incompatible with this game version."
                .to_string(),
        );
        steps.push(
            "Check the configured Java executable and confirm it starts successfully.".to_string(),
        );
        steps.push("Use the Java version recommended by the game or launcher, preferably a 64-bit runtime.".to_string());
    }

    if title.is_empty() {
        title = "Game crashed or exited unexpectedly".to_string();
        summary =
            "The log shows a crash or unexpected exit, but it does not identify a specific cause."
                .to_string();
        causes.push("The available log details are insufficient to distinguish between runtime, mod, graphics, or installation causes.".to_string());
        steps.push("Provide the complete crash log, Java version, game version, and launcher version for further diagnosis.".to_string());
        steps.push("Retry with recent mods, custom arguments, and overlays disabled, and note whether the result changes.".to_string());
    }

    CrashDiagnosis {
        found: true,
        title,
        summary,
        likely_causes: dedupe(causes),
        next_steps: dedupe(steps),
        log_excerpt: excerpt,
    }
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(MAX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn redact_excerpt(log: &str) -> String {
    log.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.contains("password")
                || lower.contains("accesstoken")
                || lower.contains("access_token")
                || lower.contains("authorization")
                || lower.contains("bearer ")
                || lower.contains(" api_key")
                || lower.starts_with("api_key")
            {
                "[REDACTED SENSITIVE LINE]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| text.contains(pattern))
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for item in items {
        if !result.iter().any(|existing| existing == &item) {
            result.push(item);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_log(contents: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("blockline-crash-helper-{stamp}.log"));
        fs::write(&path, contents).expect("test log should be writable");
        path
    }

    #[test]
    fn detects_out_of_memory() {
        let path = temp_log("java.lang.OutOfMemoryError: Java heap space\n");
        let diagnosis = analyze_log(&path);
        fs::remove_file(path).ok();
        assert!(diagnosis.found);
        assert_eq!(diagnosis.title, "Java ran out of memory");
        assert!(diagnosis
            .next_steps
            .iter()
            .any(|step| step.contains("memory allocation")));
    }

    #[test]
    fn detects_opengl_failure() {
        let path =
            temp_log("GLFW error 65542: WGL: The driver does not appear to support OpenGL\n");
        let diagnosis = analyze_log(&path);
        fs::remove_file(path).ok();
        assert!(diagnosis.found);
        assert_eq!(diagnosis.title, "Graphics initialization failed");
        assert!(diagnosis.summary.contains("OpenGL"));
    }

    #[test]
    fn redacts_secrets_in_excerpt() {
        let path =
            temp_log("startup ok\npassword=my-secret\naccessToken: very-secret-token\nfinished\n");
        let diagnosis = analyze_log(&path);
        fs::remove_file(path).ok();
        assert!(!diagnosis.log_excerpt.contains("my-secret"));
        assert!(!diagnosis.log_excerpt.contains("very-secret-token"));
        assert!(diagnosis.log_excerpt.contains("[REDACTED SENSITIVE LINE]"));
        assert!(diagnosis.log_excerpt.contains("startup ok"));
    }
}
