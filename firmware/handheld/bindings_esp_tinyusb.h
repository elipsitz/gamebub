#include "esp_private/usb_phy.h"
#include "tinyusb.h"
#include "tinyusb_cdc_acm.h"
#include "tusb_msc_storage.h"

// Get rid of a type that bindgen is unable to handle properly.
// see https://github.com/rust-lang/rust-bindgen/issues/2179
/// <div rustbindgen replaces="cdc_desc_func_telephone_call_state_reporting_capabilities_t"></div>
struct replacement1 {
    int blank;
};

// For esp_reset_reason_set_hint
#include "esp_private/system_internal.h"

// For wake stub functions
#include "esp_sleep.h"
#include "esp_wake_stub.h"
