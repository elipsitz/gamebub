use core::ffi::{c_int, c_void};
use esp_idf_svc::{
    hal::gpio::{AnyIOPin, AnyInputPin, AnyOutputPin, Pin},
    sys::{self as esp_idf_sys, EspError},
};
use std::ffi::CString;

#[allow(unused)]
mod constants {
    use esp_idf_svc::sys;

    pub const SDMMC_HOST_FLAG_1BIT: u32 = 1 << 0;
    pub const SDMMC_HOST_FLAG_4BIT: u32 = 1 << 1;
    pub const SDMMC_HOST_FLAG_8BIT: u32 = 1 << 2;
    pub const SDMMC_HOST_FLAG_SPI: u32 = 1 << 3;
    pub const SDMMC_HOST_FLAG_DDR: u32 = 1 << 4;
    pub const SDMMC_HOST_FLAG_DEINIT_ARG: u32 = 1 << 5;

    pub const SDMMC_SLOT_NO_CD: sys::gpio_num_t = sys::gpio_num_t_GPIO_NUM_NC;
    pub const SDMMC_SLOT_NO_WP: sys::gpio_num_t = sys::gpio_num_t_GPIO_NUM_NC;
    pub const SDMMC_SLOT_WIDTH_DEFAULT: u8 = 0;
}
use constants::*;

pub struct Sdcard {
    #[allow(unused)]
    card: *mut esp_idf_sys::sdmmc_card_t,
}
unsafe impl Send for Sdcard {}
unsafe impl Sync for Sdcard {}

#[allow(clippy::too_many_arguments)]
pub fn mount_sdcard(
    path: &str,
    pin_clk: AnyOutputPin,
    pin_cmd: AnyIOPin,
    pin_d0: AnyIOPin,
    pin_d1: AnyIOPin,
    pin_d2: AnyIOPin,
    pin_d3: AnyIOPin,
    pin_cd: Option<AnyInputPin>,
) -> Result<Sdcard, EspError> {
    let host_config = esp_idf_sys::sdmmc_host_t {
        flags: SDMMC_HOST_FLAG_1BIT
            | SDMMC_HOST_FLAG_4BIT
            | SDMMC_HOST_FLAG_DDR
            | SDMMC_HOST_FLAG_DEINIT_ARG,
        slot: 1,
        max_freq_khz: esp_idf_sys::SDMMC_FREQ_HIGHSPEED as c_int,
        io_voltage: 3.3,
        init: Some(esp_idf_sys::sdmmc_host_init),
        set_bus_width: Some(esp_idf_sys::sdmmc_host_set_bus_width),
        get_bus_width: Some(esp_idf_sys::sdmmc_host_get_slot_width),
        set_bus_ddr_mode: Some(esp_idf_sys::sdmmc_host_set_bus_ddr_mode),
        set_card_clk: Some(esp_idf_sys::sdmmc_host_set_card_clk),
        set_cclk_always_on: Some(esp_idf_sys::sdmmc_host_set_cclk_always_on),
        do_transaction: Some(esp_idf_sys::sdmmc_host_do_transaction),
        io_int_enable: Some(esp_idf_sys::sdmmc_host_io_int_enable),
        io_int_wait: Some(esp_idf_sys::sdmmc_host_io_int_wait),
        command_timeout_ms: 0,
        get_real_freq: Some(esp_idf_sys::sdmmc_host_get_real_freq),
        __bindgen_anon_1: esp_idf_sys::sdmmc_host_t__bindgen_ty_1 {
            deinit_p: Some(esp_idf_sys::sdmmc_host_deinit_slot),
        },
        input_delay_phase: esp_idf_sys::sdmmc_delay_phase_t_SDMMC_DELAY_PHASE_0,
        set_input_delay: Some(esp_idf_sys::sdmmc_host_set_input_delay),
        dma_aligned_buffer: std::ptr::null_mut(),
        pwr_ctrl_handle: std::ptr::null_mut(),
        get_dma_info: Some(esp_idf_sys::sdmmc_host_get_dma_info),
        current_limit: esp_idf_sys::sdmmc_current_limit_t_SDMMC_CURRENT_LIMIT_200MA,
        driver_strength: esp_idf_sys::sdmmc_driver_strength_t_SDMMC_DRIVER_STRENGTH_B,
        is_slot_set_to_uhs1: Some(esp_idf_sys::sdmmc_host_is_slot_set_to_uhs1),
    };

    let slot_config = esp_idf_sys::sdmmc_slot_config_t {
        __bindgen_anon_1: esp_idf_sys::sdmmc_slot_config_t__bindgen_ty_1 {
            gpio_cd: pin_cd.map(|p| p.pin()).unwrap_or(SDMMC_SLOT_NO_CD),
        },
        __bindgen_anon_2: esp_idf_sys::sdmmc_slot_config_t__bindgen_ty_2 {
            gpio_wp: SDMMC_SLOT_NO_WP,
        },
        width: 4,
        flags: 0,
        clk: pin_clk.pin(),
        cmd: pin_cmd.pin(),
        d0: pin_d0.pin(),
        d1: pin_d1.pin(),
        d2: pin_d2.pin(),
        d3: pin_d3.pin(),
        d4: esp_idf_sys::gpio_num_t_GPIO_NUM_NC,
        d5: esp_idf_sys::gpio_num_t_GPIO_NUM_NC,
        d6: esp_idf_sys::gpio_num_t_GPIO_NUM_NC,
        d7: esp_idf_sys::gpio_num_t_GPIO_NUM_NC,
    };

    let mount_config = esp_idf_sys::esp_vfs_fat_sdmmc_mount_config_t {
        format_if_mount_failed: false,
        max_files: 4,
        allocation_unit_size: 16 * 1024,
        disk_status_check_enable: false,
        use_one_fat: false,
    };

    let mount_point = CString::new(path)
        .map_err(|_| EspError::from_infallible::<{ esp_idf_sys::ESP_ERR_INVALID_ARG }>())?;

    log::info!("Mounting sdcard on slot {}", host_config.slot);
    let mut card: *mut esp_idf_sys::sdmmc_card_t = std::ptr::null_mut();
    let sdmmc_mount_result = unsafe {
        esp_idf_sys::esp_vfs_fat_sdmmc_mount(
            mount_point.as_ptr(),
            &host_config,
            &slot_config as *const esp_idf_sys::sdmmc_slot_config_t as *const c_void,
            &mount_config,
            &mut card,
        )
    };
    EspError::convert(sdmmc_mount_result)?;

    // Configure TinyUSB MSC driver with the sdcard. This doesn't actually expose it
    // over USB, but it configures it for if/when we actually do install tinyusb.
    let tinyusb_msc_sdmmc_config = esp_idf_sys::tinyusb_msc_storage_config_t {
        medium: esp_idf_sys::tinyusb_msc_storage_config_t__bindgen_ty_1 { card },
        fat_fs: esp_idf_sys::tinyusb_msc_fatfs_config_t {
            base_path: std::ptr::null_mut(),
            config: mount_config,
            do_not_format: true,
            format_flags: 0,
        },
        mount_point: 0,
    };
    let tinyusb_result = unsafe {
        esp_idf_sys::tinyusb_msc_new_storage_sdmmc(&tinyusb_msc_sdmmc_config, std::ptr::null_mut())
    };
    EspError::convert(tinyusb_result)?;

    Ok(Sdcard { card })
}
