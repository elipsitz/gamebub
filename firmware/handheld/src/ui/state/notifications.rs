use std::{cell::RefCell, rc::Rc, time::Duration};

use slint::{ComponentHandle, TimerMode};

use super::super::slint::Backend;
use super::UiState;

/// Down time between showing two notifications
const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub struct Notification {
    pub text: String,
    pub duration: Duration,
}

impl Notification {
    pub fn new_short(text: String) -> Notification {
        Notification {
            text,
            duration: Duration::from_millis(2000),
        }
    }

    pub fn new_long(text: String) -> Notification {
        Notification {
            text,
            duration: Duration::from_millis(3500),
        }
    }

    pub fn new_alert(text: String) -> Notification {
        Notification {
            text,
            duration: Duration::from_millis(5000),
        }
    }
}

impl UiState {
    pub fn queue_notification(&mut self, state: Rc<RefCell<Self>>, notification: Notification) {
        if self.notification_active {
            self.notification_queue.push_back(notification);
            return;
        }

        self.notification_active = true;
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_notification_visible(true);
        backend.set_notification_text(notification.text.into());

        // First timer: showing the
        self.notification_timer
            .start(TimerMode::SingleShot, notification.duration, move || {
                let inner = state.borrow_mut();
                let root = inner.root.unwrap();
                let backend = root.global::<Backend>();
                backend.set_notification_visible(false);

                // After a delay, show the next one.
                let state = state.clone();
                inner
                    .notification_timer
                    .start(TimerMode::SingleShot, INTERVAL, move || {
                        let mut inner = state.borrow_mut();
                        inner.notification_active = false;

                        if let Some(next) = inner.notification_queue.pop_front() {
                            inner.queue_notification(state.clone(), next);
                        }
                    });
            })
    }
}
