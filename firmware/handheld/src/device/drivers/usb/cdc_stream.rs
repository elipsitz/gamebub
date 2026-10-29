use esp_idf_svc::hal::task::notification::{Notification, Notifier};
use esp_idf_svc::sys::{self as esp_idf_sys, EspError};
use std::{
    io::{Read, Write},
    num::NonZero,
    sync::{Arc, Mutex},
    time::Duration,
};

static READ_NOTIFIER: Mutex<Option<Arc<Notifier>>> = Mutex::new(None);

pub struct CdcStream {
    interface: u32,
    rx_notification: Notification,
    timeout: Duration,
}

impl CdcStream {
    pub fn new(interface: u32) -> Self {
        let rx_notification = Notification::new();
        *READ_NOTIFIER.lock().unwrap() = Some(rx_notification.notifier());
        Self {
            interface,
            rx_notification,
            timeout: Duration::from_millis(1000),
        }
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut num = 0usize;
        let result = unsafe {
            esp_idf_svc::sys::tinyusb_cdcacm_read(
                self.interface,
                buf.as_mut_ptr(),
                buf.len(),
                &mut num,
            )
        };
        EspError::convert(result).map_err(std::io::Error::other)?;
        Ok(num)
    }
}

impl Read for CdcStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0;
        loop {
            let mut num = 0usize;
            let result = unsafe {
                esp_idf_svc::sys::tinyusb_cdcacm_read(
                    self.interface,
                    buf.as_mut_ptr().add(read),
                    buf.len() - read,
                    &mut num,
                )
            };
            EspError::convert(result).map_err(std::io::Error::other)?;
            read += num;

            if read == buf.len() {
                break;
            }

            // Wait for next notification.
            self.rx_notification.wait_any();
        }
        Ok(read)
    }
}

impl Write for CdcStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let num = unsafe {
            esp_idf_svc::sys::tinyusb_cdcacm_write_queue(self.interface, buf.as_ptr(), buf.len())
        };
        if num == 0 {
            self.flush()?;
        }
        Ok(num)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let result = unsafe {
            esp_idf_svc::sys::tinyusb_cdcacm_write_flush(
                self.interface,
                self.timeout.as_millis() as u32,
            )
        };
        EspError::convert(result).map_err(std::io::Error::other)
    }
}

pub unsafe extern "C" fn cdc_stream_callback_rx(
    itf: i32,
    _event: *mut esp_idf_sys::cdcacm_event_t,
) {
    if itf == 1 {
        let notifier = READ_NOTIFIER.lock().unwrap();
        if let Some(notifier) = notifier.as_ref() {
            unsafe {
                notifier.notify_and_yield(NonZero::new(1).unwrap());
            }
        }
    }
}
