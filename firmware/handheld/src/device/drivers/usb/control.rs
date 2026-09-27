use esp_idf_svc::sys;

/// Maximum control transfer data payload
const BUFFER_SIZE: usize = 512;

/// Interface number of the vendor control interface (hard-coded)
const VENDOR_INTERFACE: u16 = 0;

static mut BUFFER: [u8; BUFFER_SIZE] = [0; BUFFER_SIZE];

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Direction {
    Out,
    In,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum RequestType {
    Standard,
    Class,
    Vendor,
    Reserved,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Recipient {
    Device,
    Interface,
    Endpoint,
    Other,
    Reserved,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Request {
    pub direction: Direction,
    pub request_type: RequestType,
    pub recipient: Recipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl From<sys::tusb_control_request_t> for Request {
    fn from(value: sys::tusb_control_request_t) -> Self {
        let raw_type = unsafe { value.__bindgen_anon_1.bmRequestType };
        let direction = match (raw_type >> 7) & 0b1 {
            0 => Direction::Out,
            1 => Direction::In,
            _ => unreachable!(),
        };
        let request_type = match (raw_type >> 5) & 0b11 {
            0 => RequestType::Standard,
            1 => RequestType::Class,
            2 => RequestType::Vendor,
            3 => RequestType::Reserved,
            _ => unreachable!(),
        };
        let recipient = match raw_type & 0b11111 {
            0 => Recipient::Device,
            1 => Recipient::Interface,
            2 => Recipient::Endpoint,
            3 => Recipient::Other,
            _ => Recipient::Reserved,
        };
        Request {
            direction,
            request_type,
            recipient,
            request: value.bRequest,
            value: value.wValue,
            index: value.wIndex,
            length: value.wLength,
        }
    }
}

#[no_mangle]
pub extern "C" fn tud_vendor_control_xfer_cb(
    rhport: u8,
    stage: u8,
    raw_request: *const sys::tusb_control_request_t,
) -> bool {
    let request = Request::from(unsafe { *raw_request });
    assert!(request.request_type == RequestType::Vendor);

    if request.recipient != Recipient::Interface || request.index != VENDOR_INTERFACE {
        return false;
    }

    // This is only used within this callback, which is called from the TinyUSB task.
    #[allow(static_mut_refs)]
    let buffer = unsafe { &mut BUFFER };
    match stage as u32 {
        sys::CONTROL_STAGE_SETUP => {
            // The setup packet has been transferred.
            // The request could be aborted at this point.
            match request.direction {
                Direction::In => {
                    let result = crate::control::handle_control_in(&request, buffer);
                    if let Ok(out) = result {
                        unsafe {
                            sys::tud_control_xfer(
                                rhport,
                                raw_request,
                                out.as_ptr().cast_mut().cast(),
                                out.len() as u16,
                            );
                        }
                    } else {
                        return false;
                    }
                }
                Direction::Out => {
                    if request.length as usize > buffer.len() {
                        return false;
                    }
                    if request.length == 0 {
                        // No data phase, call the callback now.
                        let result = crate::control::handle_control_out(&request, &[]);
                        if result.is_err() {
                            return false;
                        }
                    }
                    unsafe {
                        sys::tud_control_xfer(
                            rhport,
                            raw_request,
                            buffer.as_mut_ptr().cast(),
                            request.length,
                        );
                    }
                }
            }
            true
        }
        sys::CONTROL_STAGE_DATA => {
            // The data has been transferred (in or out).
            // The request could be aborted at this point.
            match request.direction {
                Direction::In => true,
                Direction::Out => crate::control::handle_control_out(
                    &request,
                    &buffer[0..request.length as usize],
                )
                .is_ok(),
            }
        }
        sys::CONTROL_STAGE_ACK => {
            // The transaction has been acked and completed.
            crate::control::handle_control_complete(&request);
            true
        }
        _ => false,
    }
}

#[no_mangle]
pub extern "C" fn tud_vendor_rx_cb(itf: u8, buffer: *const u8, bufsize: u16) {
    if (itf as u16) != VENDOR_INTERFACE {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(buffer, bufsize as usize) };
    crate::control::handle_bulk_in(data);
}
