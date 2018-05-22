use std::ffi::CStr;
use std::fmt;
use std::os::raw::c_char;

/// A Janus transaction ID. Used to correlate signalling requests and responses.
#[derive(Debug)]
pub struct TransactionId(pub *mut c_char);

unsafe impl Send for TransactionId {}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            if self.0.is_null() {
                f.write_str("<null>")
            } else {
                let contents = CStr::from_ptr(self.0);
                f.write_str(&contents.to_string_lossy())
            }
        }
    }
}
