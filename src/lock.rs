//! Lock screen state machine.
//!
//! Manages the lock screen UI state: password input, PAM verification,
//! shake animation on wrong password, and random style selection.
//! PAM verification runs on a background thread to avoid blocking the compositor.

use crate::auth;
use std::sync::{Arc, Mutex};
use tracing::info;

/// Random lock screen style count (0..STYLE_COUNT).
const STYLE_COUNT: u8 = 5;

/// Shared auth result between compositor thread and PAM thread.
struct AuthResult {
    done: bool,
    success: bool,
}

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
    /// Pending async PAM verification result.
    auth_result: Option<Arc<Mutex<AuthResult>>>,
    /// Timestamp of the last unlock (prevents Escape key-repeat oscillation).
    pub last_unlock: Option<std::time::Instant>,
    /// Timestamp when the compositor started (prevents spurious lock during init).
    pub startup: std::time::Instant,
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
            auth_result: None,
            last_unlock: None,
            startup: std::time::Instant::now(),
        }
    }

    /// Activate the lock screen.
    pub fn lock(&mut self, pointer_x: f64) {
        // Startup guard: 忽略启动后 5 秒内的 lock 请求（防止 GDM/input 初始化干扰）
        if self.startup.elapsed().as_millis() < 5000 {
            info!("🔒 Lock request ignored (startup guard, {}ms)", self.startup.elapsed().as_millis());
            return;
        }
        info!("🔒 Locking screen");
        self.locked = true;
        self.input.clear();
        self.time = Some(std::time::Instant::now());
        self.wrong = false;
        self.shake = None;
        self.auth_result = None;
        // Random style selection using multiple entropy sources
        self.style = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis() as u64
            ^ std::process::id() as u64
            ^ (pointer_x as u64).wrapping_mul(7919)) as u8
            % STYLE_COUNT;
    }

    /// Check if async PAM verification has completed.
    /// Returns Some(true) if unlocked, Some(false) if wrong password, None if still pending.
    pub fn poll_unlock(&mut self) -> Option<bool> {
        let result = self.auth_result.as_ref()?;
        let guard = result.lock().unwrap();
        if guard.done {
            let success = guard.success;
            drop(guard);
            self.auth_result = None;
            if success {
                info!("🔓 Screen unlocked (async PAM)");
                self.locked = false;
                self.input.clear();
                self.wrong = false;
                self.shake = None;
            } else {
                info!("🔒 Wrong password (async PAM)");
                self.input.clear();
                self.wrong = true;
                self.shake = Some(std::time::Instant::now());
            }
            Some(success)
        } else {
            None
        }
    }

    /// Returns true if a PAM verification is currently in progress.
    pub fn is_authenticating(&self) -> bool {
        self.auth_result.is_some()
    }

    /// Attempt to unlock with the current input buffer (async).
    /// Starts PAM verification on a background thread.
    pub fn try_unlock(&mut self) {
        // Don't start another auth if one is already pending
        if self.auth_result.is_some() {
            return;
        }
        let username = std::env::var("USER").unwrap_or_default();
        let password = self.input.clone();
        let result = Arc::new(Mutex::new(AuthResult {
            done: false,
            success: false,
        }));
        self.auth_result = Some(result.clone());
        std::thread::spawn(move || {
            let success = auth::verify_password(&username, &password);
            let mut guard = result.lock().unwrap();
            guard.done = true;
            guard.success = success;
        });
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
