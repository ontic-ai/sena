#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliTabKind {
    Diag,
    Config,
    Actors,
    Resources,
}

impl CliTabKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "diag" => Some(Self::Diag),
            "config" => Some(Self::Config),
            "actors" => Some(Self::Actors),
            "resources" | "res" => Some(Self::Resources),
            _ => None,
        }
    }

    pub(crate) fn as_arg(self) -> &'static str {
        match self {
            Self::Diag => "diag",
            Self::Config => "config",
            Self::Actors => "actors",
            Self::Resources => "resources",
        }
    }

}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliWindowMode {
    Live,
    LegacyConfig,
    Tab(CliTabKind),
}

pub fn parse_window_mode(args: &[String]) -> Result<CliWindowMode, String> {
    let mut config_mode = false;
    let mut tab = None;
    let mut iter = args.iter().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                config_mode = true;
            }
            "--tab" => {
                let Some(name) = iter.next() else {
                    return Err("missing tab name after --tab".to_string());
                };
                tab = Some(
                    CliTabKind::parse(name)
                        .ok_or_else(|| format!("unknown tab '{}': expected diag, config, actors, or resources", name))?,
                );
            }
            _ => {}
        }
    }

    if let Some(tab) = tab {
        Ok(CliWindowMode::Tab(tab))
    } else if config_mode {
        Ok(CliWindowMode::LegacyConfig)
    } else {
        Ok(CliWindowMode::Live)
    }
}

#[cfg(test)]
mod tests {
    use super::{CliTabKind, CliWindowMode, parse_window_mode};

    #[test]
    fn parse_window_mode_defaults_to_live() {
        let args = vec!["sena-cli".to_string()];

        assert_eq!(parse_window_mode(&args), Ok(CliWindowMode::Live));
    }

    #[test]
    fn parse_window_mode_supports_tab_and_legacy_config() {
        let tab_args = vec![
            "sena-cli".to_string(),
            "--tab".to_string(),
            "resources".to_string(),
        ];
        let config_args = vec!["sena-cli".to_string(), "--config".to_string()];

        assert_eq!(
            parse_window_mode(&tab_args),
            Ok(CliWindowMode::Tab(CliTabKind::Resources))
        );
        assert_eq!(
            parse_window_mode(&config_args),
            Ok(CliWindowMode::LegacyConfig)
        );
    }

    #[test]
    fn parse_window_mode_rejects_unknown_tab_names() {
        let args = vec![
            "sena-cli".to_string(),
            "--tab".to_string(),
            "unknown".to_string(),
        ];

        assert!(parse_window_mode(&args).is_err());
    }
}