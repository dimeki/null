use libloading::{Library, Symbol};
use std::ffi::{c_char, c_double, c_int, c_void, CString};
use std::path::Path;

const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_END_FILE: c_int = 7;
const END_REASON_EOF: c_int = 0;
const END_REASON_ERROR: c_int = 4;

#[repr(C)]
struct Event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

pub enum PlayerEvent {
    /// Event queue is empty
    Idle,
    /// An event we don't act on
    Other,
    /// The file played to its end
    Eof,
    /// The file failed to play
    Error,
    Shutdown,
}

pub struct Mpv {
    lib: Library,
    ctx: *mut c_void,
}

impl Mpv {
    pub fn new(dll: &Path) -> Result<Self, String> {
        let lib = unsafe { Library::new(dll) }.map_err(|e| format!("load {}: {e}", dll.display()))?;
        let ctx = unsafe {
            let create: Symbol<unsafe extern "C" fn() -> *mut c_void> =
                lib.get(b"mpv_create").map_err(|e| e.to_string())?;
            create()
        };
        if ctx.is_null() {
            return Err("mpv_create failed".into());
        }
        let mpv = Mpv { lib, ctx };
        for (k, v) in [
            ("video", "no"),
            ("audio-display", "no"),
            ("ytdl", "no"),
            ("idle", "yes"),
            ("terminal", if std::env::var_os("YTL_DEBUG").is_some() { "yes" } else { "no" }),
            ("msg-level", "all=info"),
            ("cache", "yes"),
            ("demuxer-max-bytes", "4MiB"),
            ("demuxer-max-back-bytes", "1MiB"),
        ] {
            mpv.set_option(k, v)?;
        }
        let rc = unsafe {
            let init: Symbol<unsafe extern "C" fn(*mut c_void) -> c_int> =
                mpv.lib.get(b"mpv_initialize").map_err(|e| e.to_string())?;
            init(mpv.ctx)
        };
        if rc < 0 {
            return Err(format!("mpv_initialize failed ({rc})"));
        }
        Ok(mpv)
    }

    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (CString::new(name).unwrap(), CString::new(value).unwrap());
        let rc = unsafe {
            let f: Symbol<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int> =
                self.lib.get(b"mpv_set_option_string").map_err(|e| e.to_string())?;
            f(self.ctx, n.as_ptr(), v.as_ptr())
        };
        if rc < 0 {
            return Err(format!("option {name}={value} failed ({rc})"));
        }
        Ok(())
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe {
            let f: Symbol<unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> c_int> =
                self.lib.get(b"mpv_command").map_err(|e| e.to_string())?;
            f(self.ctx, ptrs.as_mut_ptr())
        };
        if rc < 0 {
            return Err(format!("command {:?} failed ({rc})", args));
        }
        Ok(())
    }

    /// Loads `file`, optionally starting `start_secs` into it
    pub fn load(&self, file: &Path, start_secs: Option<f64>) -> Result<(), String> {
        let path = file.to_string_lossy();
        match start_secs {
            Some(s) => self.command(&["loadfile", &path, "replace", "-1", &format!("start={s:.2}")]),
            None => self.command(&["loadfile", &path, "replace"]),
        }
    }

    pub fn property_f64(&self, name: &str) -> Option<f64> {
        let n = CString::new(name).unwrap();
        let mut out: c_double = 0.0;
        let rc = unsafe {
            let f: Symbol<unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int> =
                self.lib.get(b"mpv_get_property").ok()?;
            f(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut out as *mut c_double as *mut c_void)
        };
        (rc >= 0).then_some(out)
    }

    pub fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (CString::new(name).unwrap(), CString::new(value).unwrap());
        let rc = unsafe {
            let f: Symbol<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int> =
                self.lib.get(b"mpv_set_property_string").map_err(|e| e.to_string())?;
            f(self.ctx, n.as_ptr(), v.as_ptr())
        };
        if rc < 0 {
            return Err(format!("property {name} failed ({rc})"));
        }
        Ok(())
    }

    pub fn wait_event(&self, timeout_secs: f64) -> PlayerEvent {
        unsafe {
            let f: Symbol<unsafe extern "C" fn(*mut c_void, c_double) -> *mut Event> =
                self.lib.get(b"mpv_wait_event").expect("mpv_wait_event");
            let ev = f(self.ctx, timeout_secs);
            if ev.is_null() {
                return PlayerEvent::Idle;
            }
            match (*ev).event_id {
                EVENT_NONE => PlayerEvent::Idle,
                EVENT_SHUTDOWN => PlayerEvent::Shutdown,
                EVENT_END_FILE => {
                    // mpv_event_end_file starts with `int reason`; replacing a
                    // file also raises END_FILE (reason STOP), which we ignore
                    let reason = if (*ev).data.is_null() { -1 } else { *((*ev).data as *const c_int) };
                    match reason {
                        END_REASON_EOF => PlayerEvent::Eof,
                        END_REASON_ERROR => PlayerEvent::Error,
                        _ => PlayerEvent::Other,
                    }
                }
                _ => PlayerEvent::Other,
            }
        }
    }

}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            if let Ok(f) = self.lib.get::<unsafe extern "C" fn(*mut c_void)>(b"mpv_terminate_destroy") {
                f(self.ctx);
            }
        }
    }
}
