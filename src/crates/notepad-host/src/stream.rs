//! An in-memory `IBStream`.
//!
//! A host has to hand the plugin somewhere to write its state. A DAW would use
//! a chunk of its project file; here it is a `Vec<u8>` we can inspect, which is
//! what lets a test assert on exactly what the plugin persisted.

use std::ffi::c_void;
use std::sync::Mutex;

use vst3::{Class, Steinberg::*};

pub struct MemoryStream {
    inner: Mutex<Inner>,
}

struct Inner {
    data: Vec<u8>,
    pos: usize,
}

impl MemoryStream {
    pub fn new() -> MemoryStream {
        MemoryStream {
            inner: Mutex::new(Inner {
                data: Vec::new(),
                pos: 0,
            }),
        }
    }

    pub fn with_data(data: Vec<u8>) -> MemoryStream {
        MemoryStream {
            inner: Mutex::new(Inner { data, pos: 0 }),
        }
    }

    /// A copy of everything written so far.
    pub fn data(&self) -> Vec<u8> {
        self.inner.lock().map(|i| i.data.clone()).unwrap_or_default()
    }

    /// Rewind, so the plugin reads from the beginning.
    pub fn rewind(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pos = 0;
        }
    }
}

impl Default for MemoryStream {
    fn default() -> Self {
        MemoryStream::new()
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_read: *mut int32,
    ) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return kInternalError;
        };
        let available = inner.data.len().saturating_sub(inner.pos);
        let n = available.min(num_bytes as usize);
        if n > 0 {
            std::ptr::copy_nonoverlapping(inner.data[inner.pos..].as_ptr(), buffer as *mut u8, n);
            inner.pos += n;
        }
        if !num_bytes_read.is_null() {
            *num_bytes_read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_written: *mut int32,
    ) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return kInternalError;
        };
        let n = num_bytes as usize;
        let src = std::slice::from_raw_parts(buffer as *const u8, n);
        let pos = inner.pos;
        if pos + n > inner.data.len() {
            inner.data.resize(pos + n, 0);
        }
        inner.data[pos..pos + n].copy_from_slice(src);
        inner.pos += n;
        if !num_bytes_written.is_null() {
            *num_bytes_written = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let Ok(mut inner) = self.inner.lock() else {
            return kInternalError;
        };
        let len = inner.data.len() as i64;

        // The generated seek-mode constants are `i32` on some platforms and
        // `u32` on others, so both sides are widened before comparing rather
        // than matched directly.
        let mode = mode as i64;
        let seek_set = IBStream_::IStreamSeekMode_::kIBSeekSet as i64;
        let seek_cur = IBStream_::IStreamSeekMode_::kIBSeekCur as i64;
        let seek_end = IBStream_::IStreamSeekMode_::kIBSeekEnd as i64;

        let base = if mode == seek_set {
            0
        } else if mode == seek_cur {
            inner.pos as i64
        } else if mode == seek_end {
            len
        } else {
            return kInvalidArgument;
        };
        let target = (base + pos).clamp(0, len);
        inner.pos = target as usize;
        if !result.is_null() {
            *result = target;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        let Ok(inner) = self.inner.lock() else {
            return kInternalError;
        };
        if !pos.is_null() {
            *pos = inner.pos as int64;
        }
        kResultOk
    }
}
