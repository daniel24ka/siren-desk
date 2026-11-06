//! HTTP over WinHTTP, built into Windows: the system's own certificates and proxy, keep-alive, no TLS library to carry.
//! Every wait has a limit: no source thread can hang past it.

use std::ffi::c_void;
use std::time::Instant;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{GetLastError, SYSTEMTIME};
use windows::Win32::Networking::WinHttp::*;

/// The name this program goes by on the network: what it is, not what it is not
pub const USER_AGENT: &str = "SirenDesk/1.0 (personal desktop alert display)";

/// Home Front Command's site answers only what looks like its own page asking (403 to anything else
/// since Nov 3 2026). These are the headers its page sends (as t0mer/Redalert 4673330 and amitfin/oref_alert):
/// they go to the official base only, never to Tzofar.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
pub const BROWSER_REFERER: &str = "https://www.oref.org.il/heb";
pub const BROWSER_ACCEPT: &str = "application/json, text/plain, */*";

/// The extra request headers for a base: none for a plain one; the site's own page's for a browser-like one
pub fn request_headers(base: &Base, if_none_match: Option<&str>) -> String {
    let mut h = String::new();
    if base.browser {
        h.push_str(&format!("User-Agent: {BROWSER_USER_AGENT}\r\n"));
        h.push_str(&format!("Referer: {BROWSER_REFERER}\r\n"));
        h.push_str(&format!("Accept: {BROWSER_ACCEPT}\r\n"));
        h.push_str("Accept-Language: he-IL,he;q=0.9,en;q=0.8\r\n");
        h.push_str("X-Requested-With: XMLHttpRequest\r\n");
        // (the CDN in front of the file must not hand back a copy it has held)
        h.push_str("Cache-Control: no-cache\r\n");
        h.push_str("Pragma: no-cache\r\n");
    }
    if let Some(tag) = if_none_match {
        h.push_str(&format!("If-None-Match: {tag}\r\n"));
    }
    h
}

#[derive(Clone, Debug)]
pub struct Base {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// ask as the site's own page does (the official base only)
    pub browser: bool,
}

impl Base {
    pub fn parse(url: &str) -> Base {
        let (tls, rest) = match url.strip_prefix("https://") {
            Some(r) => (true, r),
            None => (false, url.strip_prefix("http://").unwrap_or(url)),
        };
        let rest = rest.trim_end_matches('/');
        match rest.split_once(':') {
            Some((h, p)) => Base { host: h.to_string(), port: p.parse().unwrap_or(if tls { 443 } else { 80 }), tls, browser: false },
            None => Base { host: rest.to_string(), port: if tls { 443 } else { 80 }, tls, browser: false },
        }
    }

    /// The same base, asked the way the site's own page asks
    pub fn as_browser(mut self) -> Base {
        self.browser = true;
        self
    }
}

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// how long a cache has been holding this answer (seconds)
    pub age: Option<u32>,
    pub etag: Option<String>,
    /// the server's clock
    pub date_ms: Option<i64>,
    pub took_ms: u32,
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub struct Http {
    session: Handle,
}

// (each thread owns its own session)
unsafe impl Send for Http {}

fn last_error(what: &str) -> String {
    let code = unsafe { GetLastError().0 };
    let name = match code {
        12002 => "timeout",
        12007 => "name not resolved",
        12029 => "cannot connect",
        12030 => "connection lost",
        12152 => "bad answer",
        12175 => "certificate",
        _ => "",
    };
    format!("{what}: {code} {name}").trim().to_string()
}

fn query_text(req: &Handle, level: u32) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 256];
        let mut len = (buf.len() * 2) as u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(req.0, level, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut c_void), &mut len, &mut index).ok()?;
        Some(String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]))
    }
}

fn query_date(req: &Handle) -> Option<i64> {
    unsafe {
        let mut st = SYSTEMTIME::default();
        let mut len = std::mem::size_of::<SYSTEMTIME>() as u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_DATE | WINHTTP_QUERY_FLAG_SYSTEMTIME,
            PCWSTR::null(),
            Some(&mut st as *mut SYSTEMTIME as *mut c_void),
            &mut len,
            &mut index,
        )
        .ok()?;
        let mut ft = windows::Win32::Foundation::FILETIME::default();
        windows::Win32::System::Time::SystemTimeToFileTime(&st, &mut ft).ok()?;
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        Some((ticks / 10_000) as i64 - 11_644_473_600_000)
    }
}

impl Http {
    pub fn new(timeout_ms: i32) -> Result<Http, String> {
        unsafe {
            let agent = HSTRING::from(USER_AGENT);
            let session = WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
            if session.is_null() {
                return Err(last_error("WinHttpOpen"));
            }
            let session = Handle(session);
            WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms).map_err(|e| e.to_string())?;
            // gzip and deflate: the larger feed comes in a fraction of its size
            let flags: u32 = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
            let _ = WinHttpSetOption(Some(session.0 as *const c_void), WINHTTP_OPTION_DECOMPRESSION, Some(&flags.to_ne_bytes()));
            Ok(Http { session })
        }
    }

    pub fn get(&self, base: &Base, path: &str, if_none_match: Option<&str>) -> Result<Response, String> {
        let started = Instant::now();
        unsafe {
            let host = HSTRING::from(base.host.as_str());
            let connect = Handle(WinHttpConnect(self.session.0, &host, base.port, 0));
            if connect.0.is_null() {
                return Err(last_error("connect"));
            }
            let verb = HSTRING::from("GET");
            let object = HSTRING::from(path);
            let flags = if base.tls { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
            let req = Handle(WinHttpOpenRequest(connect.0, &verb, &object, PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
            if req.0.is_null() {
                return Err(last_error("request"));
            }
            // (added to the request, each replacing one of the same name: the session's own User-Agent too)
            let text = request_headers(base, if_none_match);
            if !text.is_empty() {
                let headers: Vec<u16> = text.encode_utf16().collect();
                if WinHttpAddRequestHeaders(req.0, &headers, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE).is_err() {
                    return Err(last_error("headers"));
                }
            }
            let sent = WinHttpSendRequest(req.0, None, None, 0, 0, 0);
            if sent.is_err() {
                return Err(last_error("send"));
            }
            if WinHttpReceiveResponse(req.0, std::ptr::null_mut()).is_err() {
                return Err(last_error("receive"));
            }
            let mut status = 0u32;
            let mut len = 4u32;
            let mut index = 0u32;
            WinHttpQueryHeaders(
                req.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut c_void),
                &mut len,
                &mut index,
            )
            .map_err(|e| format!("status: {e}"))?;
            let mut body = Vec::with_capacity(4096);
            loop {
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(req.0, &mut avail).is_err() {
                    return Err(last_error("read"));
                }
                if avail == 0 {
                    break;
                }
                let at = body.len();
                body.resize(at + avail as usize, 0u8);
                let mut got = 0u32;
                if WinHttpReadData(req.0, body[at..].as_mut_ptr() as *mut c_void, avail, &mut got).is_err() {
                    return Err(last_error("read"));
                }
                body.truncate(at + got as usize);
                if got == 0 {
                    break;
                }
                if body.len() > 32 * 1024 * 1024 {
                    return Err("answer too large".into());
                }
            }
            Ok(Response {
                status,
                body,
                age: query_text(&req, WINHTTP_QUERY_AGE).and_then(|s| s.trim().parse().ok()),
                etag: query_text(&req, WINHTTP_QUERY_ETAG),
                date_ms: query_date(&req),
                took_ms: started.elapsed().as_millis() as u32,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_official_base_asks_as_a_browser() {
        let tz = Base::parse("https://api.tzevaadom.co.il");
        assert!(!tz.browser);
        assert_eq!(request_headers(&tz, None), "");
        assert_eq!(request_headers(&tz, Some("\"x\"")), "If-None-Match: \"x\"\r\n");
        let off = Base::parse("https://www.oref.org.il/").as_browser();
        assert_eq!((off.host.as_str(), off.port, off.tls), ("www.oref.org.il", 443, true));
        let h = request_headers(&off, None);
        assert!(h.contains("Referer: https://www.oref.org.il/heb\r\n"));
        assert!(h.contains("Accept: application/json, text/plain, */*\r\n"));
        assert!(h.contains("X-Requested-With: XMLHttpRequest\r\n"));
        assert!(h.contains("User-Agent: Mozilla/5.0 "));
        assert!(!h.contains("SirenDesk"));
        assert!(h.lines().all(|l| l.is_empty() || l.contains(": ")));
    }
}
