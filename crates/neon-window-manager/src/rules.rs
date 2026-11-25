//! Basic configurable window rules engine for **NEON DE**.

use crate::{geometry::Size, state_machine::WindowMode};

/// Matching predicate against a window's Wayland `app_id` and `title`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowMatcher {
    pub app_id_exact: Option<String>,
    pub app_id_contains: Option<String>,
    pub title_contains: Option<String>,
}

impl WindowMatcher {
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        if let Some(expected) = &self.app_id_exact
            && app_id != Some(expected.as_str())
        {
            return false;
        }
        if let Some(needle) = &self.app_id_contains {
            let Some(actual) = app_id else {
                return false;
            };
            if !actual.to_lowercase().contains(&needle.to_lowercase()) {
                return false;
            }
        }
        if let Some(needle) = &self.title_contains {
            let Some(actual) = title else {
                return false;
            };
            if !actual.to_lowercase().contains(&needle.to_lowercase()) {
                return false;
            }
        }
        // Require at least one matcher predicate so empty rules don't match everything accidentally.
        self.app_id_exact.is_some()
            || self.app_id_contains.is_some()
            || self.title_contains.is_some()
    }
}

/// Actions applied when a [`WindowRule`] matches a window on creation or metadata update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRuleAction {
    pub initial_mode: Option<WindowMode>,
    pub initial_size: Option<Size>,
    pub center_on_open: bool,
    pub workspace_index: Option<usize>,
    pub focus_on_map: Option<bool>,
}

/// Named window rule pairing a [`WindowMatcher`] with a [`WindowRuleAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub name: String,
    pub matcher: WindowMatcher,
    pub action: WindowRuleAction,
}

/// Evaluates ordered window rules against window metadata.
#[derive(Debug, Clone, Default)]
pub struct WindowRulesEngine {
    rules: Vec<WindowRule>,
}

impl WindowRulesEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a rules engine pre-populated with sensible NEON DE defaults.
    pub fn with_defaults() -> Self {
        let mut engine = Self::new();
        engine.add_rule(WindowRule {
            name: "center-neon-settings".into(),
            matcher: WindowMatcher {
                app_id_contains: Some("neon-settings".into()),
                ..Default::default()
            },
            action: WindowRuleAction {
                center_on_open: true,
                ..Default::default()
            },
        });
        engine.add_rule(WindowRule {
            name: "center-neon-screencast-picker".into(),
            matcher: WindowMatcher {
                app_id_contains: Some("screencast-picker".into()),
                ..Default::default()
            },
            action: WindowRuleAction {
                center_on_open: true,
                ..Default::default()
            },
        });
        engine
    }

    pub fn add_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    pub fn rules(&self) -> &[WindowRule] {
        &self.rules
    }

    /// Evaluates all matching rules in order, merging their actions cleanly.
    pub fn evaluate(&self, app_id: Option<&str>, title: Option<&str>) -> EvaluatedRules {
        let mut result = EvaluatedRules::default();
        for rule in &self.rules {
            if rule.matcher.matches(app_id, title) {
                result.matched_names.push(rule.name.clone());
                if let Some(mode) = &rule.action.initial_mode {
                    result.initial_mode = Some(mode.clone());
                }
                if let Some(size) = rule.action.initial_size {
                    result.initial_size = Some(size);
                }
                if rule.action.center_on_open {
                    result.center_on_open = true;
                }
                if let Some(ws) = rule.action.workspace_index {
                    result.workspace_index = Some(ws);
                }
                if let Some(focus) = rule.action.focus_on_map {
                    result.focus_on_map = Some(focus);
                }
            }
        }
        result
    }
}

/// Merged outcome of evaluating all matching [`WindowRule`]s for a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvaluatedRules {
    pub matched_names: Vec<String>,
    pub initial_mode: Option<WindowMode>,
    pub initial_size: Option<Size>,
    pub center_on_open: bool,
    pub workspace_index: Option<usize>,
    pub focus_on_map: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::SnapRegion;

    #[test]
    fn rules_match_app_id_and_title_and_merge_actions() {
        let mut engine = WindowRulesEngine::new();
        engine.add_rule(WindowRule {
            name: "maximize-browser".into(),
            matcher: WindowMatcher {
                app_id_exact: Some("org.mozilla.firefox".into()),
                ..Default::default()
            },
            action: WindowRuleAction {
                initial_mode: Some(WindowMode::Maximized),
                workspace_index: Some(1),
                ..Default::default()
            },
        });
        engine.add_rule(WindowRule {
            name: "snap-terminal".into(),
            matcher: WindowMatcher {
                app_id_contains: Some("terminal".into()),
                title_contains: Some("dev".into()),
                ..Default::default()
            },
            action: WindowRuleAction {
                initial_mode: Some(WindowMode::Snapped(SnapRegion::RightHalf)),
                ..Default::default()
            },
        });

        let eval = engine.evaluate(Some("org.mozilla.firefox"), Some("Home"));
        assert_eq!(eval.matched_names, vec!["maximize-browser"]);
        assert_eq!(eval.initial_mode, Some(WindowMode::Maximized));
        assert_eq!(eval.workspace_index, Some(1));

        let eval2 = engine.evaluate(
            Some("org.freedesktop.weston.wayland-terminal"),
            Some("DEV shell"),
        );
        assert_eq!(eval2.matched_names, vec!["snap-terminal"]);
        assert_eq!(
            eval2.initial_mode,
            Some(WindowMode::Snapped(SnapRegion::RightHalf))
        );
    }
}
