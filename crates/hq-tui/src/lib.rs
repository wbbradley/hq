//! Pure terminal UI state transitions and borrowed Ratatui rendering.
//!
//! This crate owns no terminal, clock, runtime, local transport, storage, or domain mutation
//! capability. A shell feeds [`UiEvent`] values into [`update`], executes returned [`UiEffect`]
//! values, and borrows the resulting [`UiModel`] for [`render`].

mod model;
mod render;

pub use model::{
    EffectId, UiConnectionState, UiEffect, UiError, UiEvent, UiFailure, UiFocus, UiInput, UiModel,
    UiRow, UiRowState, UiSection, UiSize, UiSnapshot, UiTimerKind, UiTransition, update,
};
pub use render::render;
