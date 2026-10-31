use stats_plugin_api::{
    HostApi, PluginApi, PluginDescriptor, RenderedReport, ReportRequest, ABI_VERSION,
    CAPABILITY_REPORT,
};
use std::{ffi::CStr, os::raw::c_int, ptr};

static NAME: &[u8] = b"report-json\0";
static EMPTY_VERSION: &[u8] = b"\0";

#[no_mangle]
/// # Safety
///
/// The loader supplies a valid host pointer or null and keeps the returned
/// plugin API alive for the lifetime of the loaded library.
pub unsafe extern "C" fn stats_plugin_init(_host: *const HostApi) -> *mut PluginApi {
    Box::into_raw(Box::new(PluginApi {
        descriptor: PluginDescriptor {
            abi_version: ABI_VERSION,
            capabilities: CAPABILITY_REPORT,
            name: NAME.as_ptr().cast(),
            version: EMPTY_VERSION.as_ptr().cast(),
        },
        enrich: None,
        render_report: Some(render_report),
        release_report: Some(release_report),
        shutdown: None,
    }))
}

unsafe extern "C" fn render_report(
    request: *const ReportRequest,
    output: *mut RenderedReport,
) -> c_int {
    if request.is_null() || output.is_null() {
        return 1;
    }
    let request = &*request;
    if request.format.is_null() || request.report_json.is_null() {
        return 1;
    }
    let format = CStr::from_ptr(request.format).to_bytes();
    if format != b"json" {
        return 2;
    }
    let body = CStr::from_ptr(request.report_json).to_bytes();
    set_output(output, "application/json; charset=utf-8", "json", body)
}

unsafe fn set_output(
    output: *mut RenderedReport,
    media_type: &str,
    extension: &str,
    body: &[u8],
) -> c_int {
    let Ok(media_type) = std::ffi::CString::new(media_type) else {
        return 3;
    };
    let Ok(extension) = std::ffi::CString::new(extension) else {
        return 3;
    };
    let Ok(body) = std::ffi::CString::new(body) else {
        return 3;
    };
    (*output).media_type = media_type.into_raw();
    (*output).extension = extension.into_raw();
    (*output).body = body.into_raw();
    0
}

unsafe extern "C" fn release_report(output: *mut RenderedReport) {
    if output.is_null() {
        return;
    }
    let output = &mut *output;
    for field in [
        &mut output.media_type,
        &mut output.extension,
        &mut output.body,
    ] {
        if !field.is_null() {
            drop(std::ffi::CString::from_raw(*field as *mut _));
            *field = ptr::null();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn renders_json_through_the_plugin_abi() {
        let format = CString::new("json").unwrap();
        let report =
            CString::new(r#"{"hits":7,"urls":[],"not_found":[{"url":"/.env","hits":4}]}"#).unwrap();
        let request = ReportRequest {
            format: format.as_ptr(),
            report_json: report.as_ptr(),
            section: ptr::null(),
        };
        let mut output = RenderedReport {
            media_type: ptr::null_mut(),
            extension: ptr::null_mut(),
            body: ptr::null_mut(),
        };

        let status = unsafe { render_report(&request, &mut output) };
        assert_eq!(status, 0);
        assert_eq!(
            unsafe { CStr::from_ptr(output.body) }.to_bytes(),
            report.as_bytes()
        );
        assert_eq!(
            unsafe { CStr::from_ptr(output.extension) }.to_bytes(),
            b"json"
        );
        unsafe { release_report(&mut output) };
        assert!(output.body.is_null());
    }

    #[test]
    fn rejects_null_request_strings() {
        let mut output = RenderedReport {
            media_type: ptr::null_mut(),
            extension: ptr::null_mut(),
            body: ptr::null_mut(),
        };
        let request = ReportRequest {
            format: ptr::null(),
            report_json: ptr::null(),
            section: ptr::null(),
        };
        assert_eq!(unsafe { render_report(&request, &mut output) }, 1);
    }
}
