use serde_json::from_slice;
use stats_core::Report;
use stats_plugin_api::{
    HostApi, PluginApi, PluginDescriptor, RenderedReport, ReportRequest, ABI_VERSION,
    CAPABILITY_REPORT,
};
use std::{ffi::CStr, os::raw::c_int, ptr};

static NAME: &[u8] = b"report-html\0";
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
    if CStr::from_ptr(request.format).to_bytes() != b"html" {
        return 2;
    }
    let Ok(report) = from_slice::<Report>(CStr::from_ptr(request.report_json).to_bytes()) else {
        return 3;
    };
    let body = if request.section.is_null() {
        stats_render::render_html_report(&report)
    } else {
        let Ok(section) = CStr::from_ptr(request.section).to_str() else {
            return 3;
        };
        match stats_render::render_html_section(&report, section) {
            Ok(body) => body,
            Err(_) => return 4,
        }
    };
    set_output(output, "text/html; charset=utf-8", "html", body.as_bytes())
}

unsafe fn set_output(
    output: *mut RenderedReport,
    media_type: &str,
    extension: &str,
    body: &[u8],
) -> c_int {
    let Ok(media_type) = std::ffi::CString::new(media_type) else {
        return 4;
    };
    let Ok(extension) = std::ffi::CString::new(extension) else {
        return 4;
    };
    let Ok(body) = std::ffi::CString::new(body) else {
        return 4;
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
