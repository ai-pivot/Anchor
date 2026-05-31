use std::sync::{Arc, Mutex};
use zbus::{interface, connection::Builder as ConnBuilder};

/// A notification received via D-Bus
#[derive(Debug, Clone)]
pub struct DbusNotification {
    pub app_name: String,
    pub summary: String,
    pub body: String,
}

/// Shared notification state between the D-Bus thread and the compositor
pub struct NotificationState {
    pub pending: Vec<DbusNotification>,
}

pub struct NotificationServer {
    state: Arc<Mutex<NotificationState>>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    fn notify(
        &mut self,
        app_name: &str,
        _replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        _actions: Vec<&str>,
        _hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>>,
        _expire_timeout: i32,
    ) -> u32 {
        let notif = DbusNotification {
            app_name: app_name.to_string(),
            summary: summary.to_string(),
            body: body.to_string(),
        };
        if let Ok(mut state) = self.state.lock() {
            state.pending.push(notif);
        }
        0
    }

    fn get_server_information(&mut self) -> (&str, &str, &str, &str) {
        ("anchor", "anchor", "0.1.0", "1.2")
    }

    fn get_capabilities(&mut self) -> Vec<&str> {
        vec!["body"]
    }

    fn close_notification(&mut self, _id: u32) {}
}

impl NotificationServer {
    pub fn new(state: Arc<Mutex<NotificationState>>) -> Self {
        Self { state }
    }
}

/// Start the D-Bus notification daemon in a background thread.
pub fn start_notification_daemon() -> Arc<Mutex<NotificationState>> {
    let state = Arc::new(Mutex::new(NotificationState { pending: Vec::new() }));
    let state_clone = state.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async {
            let server = NotificationServer::new(state_clone);
            let _conn = ConnBuilder::session()
                .expect("Failed to connect to session bus")
                .name("org.freedesktop.Notifications")
                .expect("Failed to register notification name")
                .serve_at("/org/freedesktop/Notifications", server)
                .expect("Failed to serve notification interface")
                .build()
                .await
                .expect("Failed to build connection");

            std::future::pending::<()>().await;
        });
    });

    state
}
