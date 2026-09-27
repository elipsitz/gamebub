pub use cdc_stream::CdcStream;
use descriptors::Descriptors;
use esp_idf_svc::sys::{self as esp_idf_sys, EspError};
use std::sync::Mutex;

mod cdc_stream;
pub mod control;
mod descriptors;

/// Current USB state
static STATE: Mutex<UsbState> = Mutex::new(UsbState {
    mode: UsbMode::SerialJtag,
    descriptors: None,
});

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum UsbMode {
    SerialJtag,
    ConsoleOnly,
    ConsoleAndVendor,
    ConsoleAndMassStorage,
    ConsoleAndSerial,
}

struct UsbState {
    mode: UsbMode,
    descriptors: Option<Box<Descriptors>>,
}

pub fn configure_usb(mode: UsbMode) -> Result<(), EspError> {
    let mut state = STATE.lock().unwrap();
    if mode == state.mode {
        return Ok(());
    }

    // Start by tearing down previous USB.
    match state.mode {
        UsbMode::SerialJtag => {}
        _ => {
            // De-initialize the console temporarily.
            let cdc_port = esp_idf_sys::tinyusb_cdcacm_itf_t_TINYUSB_CDC_ACM_0 as i32;
            let result = unsafe { esp_idf_sys::tinyusb_cdcacm_deinit(cdc_port) };
            EspError::convert(result)?;
            let result = unsafe { esp_idf_sys::tinyusb_console_deinit(cdc_port) };
            EspError::convert(result)?;
            // Uninstall the TinyUSB driver.
            let result = unsafe { esp_idf_sys::tinyusb_driver_uninstall() };
            EspError::convert(result)?;
            state.descriptors = None;
        }
    }

    // Simple case, switch to USB Serial/JTAG:
    if mode == UsbMode::SerialJtag {
        setup_usb_serial_jtag()?;
        state.mode = mode;
        return Ok(());
    }

    // Set up console CDC buffering such that:
    // * queued data will not be cleared on USB attach or reset
    // * new data doesn't overwrite old queued data when full
    unsafe {
        let mut cdc_config = esp_idf_sys::tud_cdc_configure_t::default();
        cdc_config.set_tx_persistent(1);
        cdc_config.set_tx_overwritabe_if_not_connected(0);
        esp_idf_sys::tud_cdc_configure(&cdc_config);
    }

    // Set up the descriptors
    let mut descriptors = descriptors::Builder::new();
    if mode == UsbMode::ConsoleAndVendor {
        descriptors.add_vendor_bulk();
    } else {
        descriptors.add_vendor();
    }
    descriptors.add_cdc();
    match mode {
        UsbMode::ConsoleAndMassStorage => descriptors.add_msc(),
        UsbMode::ConsoleAndSerial => descriptors.add_cdc(),
        _ => 0,
    };
    let hardware_version = crate::hwinfo::get_hardware_version();
    let mode_id = match mode {
        UsbMode::SerialJtag => 0,
        UsbMode::ConsoleOnly => 0,
        UsbMode::ConsoleAndVendor => 2,
        UsbMode::ConsoleAndMassStorage => 2,
        UsbMode::ConsoleAndSerial => 3,
    };
    let device_version = ((hardware_version.major as u16) << 8)
        | ((hardware_version.minor as u16 & 0xF) << 4)
        | mode_id;
    descriptors.set_device_version(device_version);
    let descriptors = Box::new(descriptors.build());

    // Install the TinyUSB driver
    let tinyusb_config = descriptors.tinyusb_config();
    state.descriptors = Some(descriptors);
    let result = unsafe { esp_idf_sys::tinyusb_driver_install(&tinyusb_config) };
    EspError::convert(result)?;

    // Setup first CDC interface (console)
    let cdc_port = esp_idf_sys::tinyusb_cdcacm_itf_t_TINYUSB_CDC_ACM_0;
    let acm_config = esp_idf_sys::tinyusb_config_cdcacm_t {
        cdc_port,
        callback_rx: None,
        callback_rx_wanted_char: None,
        callback_line_state_changed: None,
        callback_line_coding_changed: None,
    };
    let result = unsafe { esp_idf_sys::tinyusb_cdcacm_init(&acm_config) };
    EspError::convert(result)?;
    let result = unsafe { esp_idf_sys::tinyusb_console_init(cdc_port as i32) };
    EspError::convert(result)?;

    // Set up second CDC interface
    if mode == UsbMode::ConsoleAndSerial {
        let acm_config = esp_idf_sys::tinyusb_config_cdcacm_t {
            cdc_port: esp_idf_sys::tinyusb_cdcacm_itf_t_TINYUSB_CDC_ACM_1,
            callback_rx: Some(cdc_stream::cdc_stream_callback_rx),
            callback_rx_wanted_char: None,
            callback_line_state_changed: None,
            callback_line_coding_changed: None,
        };
        let result = unsafe { esp_idf_sys::tinyusb_cdcacm_init(&acm_config) };
        EspError::convert(result)?;
    }

    state.mode = mode;
    Ok(())
}

/// Initialize USB Serial/JTAG device
fn setup_usb_serial_jtag() -> Result<(), EspError> {
    // Re-initialize Serial/JTAG.
    let phy_config = esp_idf_sys::usb_phy_config_t {
        controller: esp_idf_sys::usb_phy_controller_t_USB_PHY_CTRL_SERIAL_JTAG,
        target: esp_idf_sys::usb_phy_target_t_USB_PHY_TARGET_INT,
        otg_mode: 0,
        otg_speed: 0,
        ext_io_conf: std::ptr::null(),
        otg_io_conf: std::ptr::null(),
    };
    let mut jtag_phy: esp_idf_sys::usb_phy_handle_t = std::ptr::null_mut();
    let result = unsafe { esp_idf_sys::usb_new_phy(&phy_config, &mut jtag_phy) };
    EspError::convert(result)
}
