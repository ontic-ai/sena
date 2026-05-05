//! Rich task inference with semantic descriptions.

use bus::events::ctp::{ContextSnapshot, EnrichedInferredTask};

/// Task inference engine that generates semantic descriptions of user activities.
pub struct TaskInferenceEngine;

impl TaskInferenceEngine {
    /// Infer the user's current task from the context snapshot.
    ///
    /// Generates semantic descriptions based on app name, window title, and
    /// behavioral signals like keystroke cadence.
    pub fn infer(&self, snapshot: &ContextSnapshot) -> Option<EnrichedInferredTask> {
        let app_name = &snapshot.active_app.app_name;
        let window_title = snapshot.active_app.window_title.as_deref();
        let cadence = &snapshot.keystroke_cadence;

        let app_lower = app_name.to_lowercase();
        let title_lower = window_title.unwrap_or_default().to_lowercase();

        // Match app and window patterns
        let (category, base_description, mut confidence): (&str, String, f32) = if app_lower
            .contains("code")
            || app_lower.contains("rustrover")
            || app_lower.contains("idea")
            || app_lower.contains("cursor")
            || app_lower.contains("vscode")
        {
            let desc = if let Some(title) = window_title {
                if title.ends_with(".rs") || title.contains(".rs ") {
                    format!("Editing Rust source code ({})", extract_filename(title))
                } else if title.ends_with(".py") || title.contains(".py ") {
                    format!("Editing Python code ({})", extract_filename(title))
                } else if title.ends_with(".js")
                    || title.ends_with(".ts")
                    || title.contains(".js ")
                    || title.contains(".ts ")
                {
                    format!(
                        "Editing JavaScript/TypeScript ({})",
                        extract_filename(title)
                    )
                } else if title.contains("main.") {
                    format!("Editing source code ({})", extract_filename(title))
                } else {
                    format!("Writing code in {}", app_name)
                }
            } else {
                format!("Writing code in {}", app_name)
            };
            ("coding", desc, 0.78)
        } else if app_lower.contains("chrome")
            || app_lower.contains("firefox")
            || app_lower.contains("edge")
            || app_lower.contains("safari")
            || app_lower.contains("browser")
        {
            let desc = if let Some(title) = window_title {
                if title_lower.contains("github.com") || title_lower.contains("github") {
                    "Reviewing code on GitHub".to_string()
                } else if title_lower.contains("docs.") || title_lower.contains("documentation") {
                    "Reading documentation".to_string()
                } else if title_lower.contains("stack overflow")
                    || title_lower.contains("stackoverflow")
                {
                    "Researching on Stack Overflow".to_string()
                } else if title_lower.contains("youtube") {
                    "Watching YouTube".to_string()
                } else {
                    format!("Browsing: {}", title)
                }
            } else {
                format!("Browsing in {}", app_name)
            };
            ("research", desc, 0.66)
        } else if app_lower.contains("word")
            || app_lower.contains("notion")
            || app_lower.contains("obsidian")
            || app_lower.contains("evernote")
            || title_lower.contains("doc")
            || title_lower.contains(".md")
        {
            let desc = if let Some(_title) = window_title {
                format!("Writing document: {}", _title)
            } else {
                format!("Writing in {}", app_name)
            };
            ("writing", desc, 0.70)
        } else if app_lower.contains("terminal")
            || app_lower.contains("powershell")
            || app_lower.contains("cmd")
            || app_lower.contains("iterm")
            || app_lower.contains("alacritty")
        {
            let desc = if let Some(_title) = window_title {
                if title_lower.contains("cargo") {
                    "Running Rust build tools".to_string()
                } else if title_lower.contains("npm") || title_lower.contains("node") {
                    "Running Node.js tools".to_string()
                } else if title_lower.contains("git") {
                    "Working with Git version control".to_string()
                } else if title_lower.contains("python") {
                    "Running Python scripts".to_string()
                } else {
                    format!("Running commands in {}", app_name)
                }
            } else {
                format!("Running commands in {}", app_name)
            };
            ("operations", desc, 0.72)
        } else if app_lower.contains("slack")
            || app_lower.contains("teams")
            || app_lower.contains("discord")
            || app_lower.contains("zoom")
        {
            (
                "communication",
                format!("Communication in {}", app_name),
                0.68,
            )
        } else if app_lower.contains("spotify")
            || app_lower.contains("music")
            || app_lower.contains("itunes")
        {
            ("media", format!("Listening to music in {}", app_name), 0.80)
        } else {
            // Unknown app
            ("general", format!("Working in {}", app_name), 0.45)
        };

        // Adjust confidence based on keystroke cadence
        if cadence.burst_detected {
            confidence += 0.08;
        }
        if cadence.events_per_minute > 140.0 {
            confidence += 0.05;
        }
        if cadence.events_per_minute < 20.0 {
            confidence -= 0.10;
        }

        // Clamp confidence to [0.0, 1.0]
        confidence = confidence.clamp(0.0, 1.0);

        Some(EnrichedInferredTask {
            category: category.to_string(),
            semantic_description: base_description,
            confidence,
        })
    }
}

/// Extract filename from window title.
fn extract_filename(title: &str) -> String {
    // Try to extract filename from title like "main.rs - project - VSCode"
    if let Some(dash_pos) = title.find(" - ") {
        title[..dash_pos].trim().to_string()
    } else if let Some(dash_pos) = title.find('—') {
        title[..dash_pos].trim().to_string()
    } else {
        title.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    fn mock_snapshot(app: &str, title: Option<&str>, cadence: f64) -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: app.to_string(),
                window_title: title.map(|s| s.to_string()),
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: cadence,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            },
            session_duration: Duration::from_secs(600),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
        }
    }

    #[test]
    fn infers_rust_code_editing() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Code", Some("main.rs - sena"), 120.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "coding");
        assert!(task.semantic_description.contains("Rust"));
        assert!(task.semantic_description.contains("main.rs"));
        assert!(task.confidence >= 0.78);
    }

    #[test]
    fn infers_python_code_editing() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("VSCode", Some("script.py - project"), 100.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "coding");
        assert!(task.semantic_description.contains("Python"));
        assert!(task.semantic_description.contains("script.py"));
    }

    #[test]
    fn infers_github_review() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Chrome", Some("Pull Request #123 - GitHub"), 60.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "research");
        assert!(task.semantic_description.contains("GitHub"));
    }

    #[test]
    fn infers_documentation_reading() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Firefox", Some("Rust Documentation"), 30.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "research");
        assert!(task.semantic_description.contains("documentation"));
    }

    #[test]
    fn infers_cargo_build() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Terminal", Some("cargo build --release"), 80.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "operations");
        assert!(task.semantic_description.contains("Rust build tools"));
    }

    #[test]
    fn infers_git_usage() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("PowerShell", Some("git commit -m"), 90.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "operations");
        assert!(task.semantic_description.contains("Git"));
    }

    #[test]
    fn infers_npm_tools() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Terminal", Some("npm install"), 70.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "operations");
        assert!(task.semantic_description.contains("Node"));
    }

    #[test]
    fn infers_writing_in_notion() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Notion", Some("Project Notes"), 85.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "writing");
        assert!(task.semantic_description.contains("Writing"));
    }

    #[test]
    fn infers_slack_communication() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Slack", Some("#general"), 50.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "communication");
        assert!(task.semantic_description.contains("Communication"));
    }

    #[test]
    fn infers_generic_unknown_app() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("UnknownApp", None, 60.0);

        let task = engine.infer(&snapshot).unwrap();

        assert_eq!(task.category, "general");
        assert!(task.semantic_description.contains("Working in UnknownApp"));
        assert!(task.confidence < 0.50);
    }

    #[test]
    fn adjusts_confidence_for_burst() {
        let engine = TaskInferenceEngine;
        let mut snapshot = mock_snapshot("Code", Some("main.rs"), 160.0);
        snapshot.keystroke_cadence.burst_detected = true;

        let task = engine.infer(&snapshot).unwrap();

        // Base 0.78 + 0.08 (burst) + 0.05 (high cadence) = 0.91
        assert!(task.confidence >= 0.90);
    }

    #[test]
    fn reduces_confidence_for_low_cadence() {
        let engine = TaskInferenceEngine;
        let snapshot = mock_snapshot("Code", Some("lib.rs"), 15.0);

        let task = engine.infer(&snapshot).unwrap();

        // Base 0.78 - 0.10 (low cadence) = 0.68
        assert!(task.confidence <= 0.70);
    }
}
