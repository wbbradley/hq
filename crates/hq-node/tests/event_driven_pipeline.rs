//! Architectural regression checks for the healthy interaction-delivery path.

const PROJECT_COMPONENT: &str = include_str!("../src/project_component.rs");
const RELAY_COMPONENT: &str = include_str!("../src/relay_component.rs");
const RELAY_MANAGER: &str = include_str!("../../hq-relay/src/manager.rs");
const HARNESS_COMPONENT: &str = include_str!("../src/harness_component.rs");
const SESSION_PUMP: &str = include_str!("../src/session_pump.rs");
const TUI_CLIENT: &str = include_str!("../src/tui_client.rs");
const TUI_SHELL: &str = include_str!("../src/tui_shell.rs");

#[test]
fn healthy_interaction_owners_have_no_recurring_poll_timer() {
    let owners = [
        ("relay component", production_source(RELAY_COMPONENT)),
        ("relay manager", production_source(RELAY_MANAGER)),
        ("project component", production_source(PROJECT_COMPONENT)),
        ("harness component", production_source(HARNESS_COMPONENT)),
        ("local session pump", production_source(SESSION_PUMP)),
        ("TUI client", production_source(TUI_CLIENT)),
        ("TUI shell", production_source(TUI_SHELL)),
    ];
    let recurring_timer_primitives = [
        "tokio::time::interval",
        "interval_at(",
        "park_timeout(",
        "thread::sleep(",
        "std::thread::sleep(",
    ];

    for (owner, source) in owners {
        for primitive in recurring_timer_primitives {
            assert!(
                !source.contains(primitive),
                "{owner} must be notification-driven; found recurring timer primitive {primitive}"
            );
        }
    }
}

#[test]
fn removed_healthy_poll_configuration_stays_removed() {
    let pipeline = [
        RELAY_COMPONENT,
        RELAY_MANAGER,
        PROJECT_COMPONENT,
        HARNESS_COMPONENT,
        SESSION_PUMP,
        TUI_CLIENT,
        TUI_SHELL,
    ]
    .join("\n");
    let removed_controls = [
        "project_poll_interval",
        "event_poll_interval",
        "park_timeout",
        "snapshot_refresh_interval",
        "snapshot_repair_interval",
    ];

    for control in removed_controls {
        assert!(
            !pipeline.contains(control),
            "removed healthy-state polling control returned: {control}"
        );
    }
}

fn production_source(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}
