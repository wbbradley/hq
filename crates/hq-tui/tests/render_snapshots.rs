//! Deterministic terminal-buffer snapshots for every responsive layout.

#![allow(clippy::expect_used)]

use hq_tui::{
    UiEffect, UiEvent, UiInput, UiModel, UiRow, UiRowState, UiSize, UiSnapshot, render, update,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

#[test]
fn wide_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        include_str!("snapshots/wide.txt"),
    );
}

#[test]
fn compact_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 72,
            height: 16,
        }),
        include_str!("snapshots/compact.txt"),
    );
}

#[test]
fn too_small_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 30,
            height: 8,
        }),
        include_str!("snapshots/too-small.txt"),
    );
}

fn assert_snapshot(model: &UiModel, expected: &str) {
    let before = model.clone();
    let viewport = model.viewport();
    let backend = TestBackend::new(viewport.width, viewport.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model))
        .expect("render buffer");
    assert_eq!(model, &before, "rendering must only borrow the model");
    assert_eq!(snapshot_text(terminal.backend().buffer()), expected);
}

fn snapshot_text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width);
    let mut output = buffer
        .content()
        .chunks(width)
        .map(|cells| {
            cells
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    output
}

fn ready_model(size: UiSize) -> UiModel {
    let started = update(UiModel::new(size), UiEvent::Started).expect("start model");
    let request = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("snapshot request");
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: UiSnapshot {
                section: hq_tui::UiSection::Inbox,
                revision: 42,
                rows: vec![
                    row("build-17", "Build release", "agent-1", UiRowState::Open),
                    row(
                        "deploy-9",
                        "Deploy production",
                        "waiting for approval",
                        UiRowState::Waiting,
                    ),
                    row(
                        "incident-4",
                        "Investigate timeout",
                        "relay needs attention",
                        UiRowState::Attention,
                    ),
                ],
            },
        },
    )
    .expect("load snapshot");
    update(loaded.model, UiEvent::Input(UiInput::NextItem))
        .expect("select second row")
        .model
}

fn row(id: &str, title: &str, detail: &str, state: UiRowState) -> UiRow {
    UiRow {
        id: id.to_owned(),
        title: title.to_owned(),
        detail: detail.to_owned(),
        state,
    }
}
