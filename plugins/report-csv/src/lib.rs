use serde_json::Value;
use stats_plugin_api::{
    HostApi, PluginApiExtended, PluginDescriptor, RenderedReport, ReportRequest, ABI_VERSION_V2,
    CAPABILITY_REPORT,
};
use std::{ffi::CStr, os::raw::c_int, ptr};

static NAME: &[u8] = b"report-csv\0";
static EMPTY_VERSION: &[u8] = b"\0";

#[no_mangle]
/// # Safety
///
/// The loader supplies a valid host pointer or null and keeps the returned
/// plugin API alive for the lifetime of the loaded library.
pub unsafe extern "C" fn stats_plugin_init_v2(_host: *const HostApi) -> *mut PluginApiExtended {
    Box::into_raw(Box::new(PluginApiExtended {
        descriptor: PluginDescriptor {
            abi_version: ABI_VERSION_V2,
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
    if CStr::from_ptr(request.format).to_bytes() != b"csv" {
        return 2;
    }
    let Ok(report) =
        serde_json::from_slice::<Value>(CStr::from_ptr(request.report_json).to_bytes())
    else {
        return 3;
    };
    let body = render_csv(&report);
    set_output(output, "text/csv; charset=utf-8", "csv", body.as_bytes())
}

fn render_csv(report: &Value) -> String {
    let mut out = String::from("section,key,hits,bytes,pages,files,visits\n");
    for key in ["hits", "bytes", "pages", "files", "visits", "unique_visits"] {
        let value = number(report, key);
        if key == "bytes" {
            out.push_str(&format!("summary,{key},,{value},,,\n"));
        } else {
            out.push_str(&format!("summary,{key},{value},,,,\n"));
        }
    }
    for section in [
        "countries",
        "regions",
        "cities",
        "asns",
        "organizations",
        "statuses",
        "status_classes",
        "errors",
        "methods",
        "urls",
        "users",
        "user_agents",
        "referers",
        "referrer_domains",
        "file_types",
        "browsers",
        "oses",
        "robots",
        "request_times",
        "virtual_hosts",
        "entry_pages",
        "exit_pages",
        "search_engines",
        "keyphrases",
        "hours",
        "dates",
        "months",
        "weekdays",
        "diagnostics",
    ] {
        if let Some(rows) = report.get(section).and_then(Value::as_array) {
            for row in rows {
                out.push_str(&format!(
                    "{},{},{},,,,\n",
                    field(section),
                    csv_field(row.get("key").and_then(Value::as_str).unwrap_or_default()),
                    number(row, "hits")
                ));
            }
        }
    }
    if let Some(rows) = report.get("bandwidth_urls").and_then(Value::as_array) {
        for row in rows {
            out.push_str(&format!(
                "bandwidth_urls,{},{},{},,,\n",
                csv_field(row.get("key").and_then(Value::as_str).unwrap_or_default()),
                number(row, "hits"),
                number(row, "bytes")
            ));
        }
    }
    if let Some(rows) = report.get("daily").and_then(Value::as_array) {
        for row in rows {
            out.push_str(&format!(
                "daily,{},{},{},{},{},{}\n",
                csv_field(row.get("date").and_then(Value::as_str).unwrap_or_default()),
                number(row, "hits"),
                number(row, "bytes"),
                number(row, "pages"),
                number(row, "files"),
                number(row, "visits")
            ));
        }
    }
    out
}

fn field(value: &str) -> String {
    csv_field(value)
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
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
