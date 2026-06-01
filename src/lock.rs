//! Lock screen state machine.
//!
//! Manages the lock screen UI state: password input, PAM verification,
//! shake animation on wrong password, and random style selection.

use crate::auth;
use tracing::info;

/// Random lock screen style count (0..STYLE_COUNT).
const STYLE_COUNT: u8 = 5;

/// Lock screen state, held inside `App`.
pub struct LockState {
    /// Whether the screen is currently locked.
    pub locked: bool,
    /// Password input buffer (cleartext, only in memory during input).
    pub input: String,
    /// Timestamp when the lock screen was activated (for animation timing).
    pub time: Option<std::time::Instant>,
    /// Timestamp of the last wrong-password shake animation trigger.
    pub shake: Option<std::time::Instant>,
    /// Whether the last password attempt was wrong (triggers red flash).
    pub wrong: bool,
    /// Random visual style index (0..STYLE_COUNT).
    pub style: u8,
}

impl LockState {
    pub fn new() -> Self {
        Self {
            locked: false,
            input: String::new(),
            time: None,
            shake: None,
            wrong: false,
            style: 0,
        }
    }

    /// Activate the lock screen.
    pub fn lock(&mut self, pointer_x: f64) {
        info!("🔒 Locking screen");
        self.locked = true;
        self.input.clear();
        self.time = Some(std::time::Instant::now());
        self.wrong = false;
        self.shake = None;
        // Random style selection using multiple entropy sources
        self.style = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis() as u64
            ^ std::process::id() as u64
            ^ (pointer_x as u64).wrapping_mul(7919)) as u8
            % STYLE_COUNT;
    }

    /// Attempt to unlock with the current input buffer.
    /// Returns `true` if successfully unlocked.
    pub fn try_unlock(&mut self) -> bool {
        let username = std::env::var("USER").unwrap_or_default();
        let password = self.input.clone();
        if auth::verify_password(&username, &password) {
            info!("🔓 Screen unlocked");
            self.locked = false;
            self.input.clear();
            self.wrong = false;
            self.shake = None;
            true
        } else {
            info!("🔒 Wrong password");
            self.input.clear();
            self.wrong = true;
            self.shake = Some(std::time::Instant::now());
            false
        }
    }

    /// Handle backspace during lock screen input.
    pub fn backspace(&mut self) {
        self.input.pop();
        self.wrong = false;
    }

    /// Append a printable character to the password buffer.
    pub fn push_char(&mut self, c: char) {
        self.input.push(c);
        self.wrong = false;
    }

    /// Clear input and reset wrong flag (Escape key).
    pub fn clear(&mut self) {
        self.input.clear();
        self.wrong = false;
    }
}
