use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;
use tauri::menu::{Menu, MenuItem, Submenu};
use tauri::{
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;

const OPEN_NEW_MENU_ID: &str = "open-new";

#[derive(Default)]
struct WindowRegistry {
    file_to_window: Mutex<HashMap<String, String>>,
    window_to_file: Mutex<HashMap<String, String>>,
    next_window_index: AtomicUsize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadMarkdownResponse {
    content: String,
    mtime_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteMarkdownResponse {
    mtime_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatMarkdownResponse {
    mtime_ms: u64,
}

fn canonical_markdown_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Markdown path cannot be empty.".to_string());
    }

    let candidate = PathBuf::from(trimmed);
    let canonical = fs::canonicalize(&candidate)
        .map_err(|err| format!("Failed to resolve markdown path: {err}"))?;

    if !canonical.is_file() {
        return Err("Path is not a file.".to_string());
    }

    let extension = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false);

    if !extension {
        return Err("Only .md files are supported.".to_string());
    }

    Ok(canonical)
}

fn modified_ms(path: &Path) -> Result<u64, String> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|err| format!("Failed to read file metadata: {err}"))?;

    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("Failed to normalize modified time: {err}"))?;

    Ok(duration.as_millis() as u64)
}

#[tauri::command]
fn read_markdown(path: String) -> Result<ReadMarkdownResponse, String> {
    let canonical = canonical_markdown_path(&path)?;
    let content = fs::read_to_string(&canonical)
        .map_err(|err| format!("Failed to read markdown file: {err}"))?;

    Ok(ReadMarkdownResponse {
        content,
        mtime_ms: modified_ms(&canonical)?,
    })
}

#[tauri::command]
fn write_markdown(path: String, content: String) -> Result<WriteMarkdownResponse, String> {
    let canonical = canonical_markdown_path(&path)?;

    fs::write(&canonical, content)
        .map_err(|err| format!("Failed to write markdown file: {err}"))?;

    Ok(WriteMarkdownResponse {
        mtime_ms: modified_ms(&canonical)?,
    })
}

#[tauri::command]
fn stat_markdown(path: String) -> Result<StatMarkdownResponse, String> {
    let canonical = canonical_markdown_path(&path)?;

    Ok(StatMarkdownResponse {
        mtime_ms: modified_ms(&canonical)?,
    })
}

fn canonical_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn capitalize_word(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    format!("{}{}", first.to_uppercase(), chars.as_str())
}

fn format_display_file_name(path: &Path) -> String {
    let normalized_path = path.to_string_lossy().replace('\\', "/");
    let raw_name = normalized_path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(normalized_path.as_str());

    let ext_index = raw_name.rfind('.');
    let base_name = if ext_index.is_some_and(|index| index > 0) {
        &raw_name[..ext_index.unwrap_or(raw_name.len())]
    } else {
        raw_name
    };

    let normalized = base_name.replace(['_', '-'], " ");
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let fallback = if raw_name.is_empty() {
        path.to_string_lossy().to_string()
    } else {
        raw_name.to_string()
    };
    let source = if collapsed.is_empty() {
        fallback
    } else {
        collapsed
    };

    source
        .split_whitespace()
        .map(capitalize_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn try_path_from_arg(raw: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with("-psn_") {
        return None;
    }

    if !trimmed.to_lowercase().ends_with(".md") {
        return None;
    }

    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        if let Some(base) = cwd {
            Some(base.join(&candidate))
        } else {
            Some(candidate)
        }
    }
}

fn is_markdown_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
}

fn push_unique_markdown_path(candidate: &Path, seen: &mut HashSet<String>, paths: &mut Vec<PathBuf>) {
    let canonical = match fs::canonicalize(candidate) {
        Ok(path) => path,
        Err(_) => return,
    };

    if !is_markdown_file(&canonical) {
        return;
    }

    let key = canonical_key(&canonical);
    if seen.insert(key) {
        paths.push(canonical);
    }
}

fn extract_markdown_paths(args: &[String], cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for raw in args {
        let Some(candidate) = try_path_from_arg(raw, cwd) else {
            continue;
        };

        push_unique_markdown_path(&candidate, &mut seen, &mut paths);
    }

    paths
}

fn focus_window(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn register_file_window(app: &AppHandle, file_key: &str, label: &str) {
    let registry = app.state::<WindowRegistry>();

    {
        let mut file_to_window = registry
            .file_to_window
            .lock()
            .expect("window registry poisoned");
        file_to_window.insert(file_key.to_string(), label.to_string());
    }

    {
        let mut window_to_file = registry
            .window_to_file
            .lock()
            .expect("window registry poisoned");
        window_to_file.insert(label.to_string(), file_key.to_string());
    }
}

fn remove_window_registration(app: &AppHandle, label: &str) {
    let registry = app.state::<WindowRegistry>();
    let maybe_file_key = {
        let mut window_to_file = registry
            .window_to_file
            .lock()
            .expect("window registry poisoned");
        window_to_file.remove(label)
    };

    if let Some(file_key) = maybe_file_key {
        let mut file_to_window = registry
            .file_to_window
            .lock()
            .expect("window registry poisoned");
        file_to_window.remove(&file_key);
    }
}

fn attach_destroy_cleanup(window: &WebviewWindow, label: String) {
    let app = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            remove_window_registration(&app, &label);
        }
    });
}

fn next_window_label(app: &AppHandle) -> String {
    let registry = app.state::<WindowRegistry>();
    let id = registry.next_window_index.fetch_add(1, Ordering::Relaxed);
    format!("md-{id}")
}

fn build_window(
    app: &AppHandle,
    label: String,
    file_path: Option<&Path>,
) -> Result<WebviewWindow, String> {
    let title = file_path
        .map(format_display_file_name)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "TodoMark".to_string());

    let mut builder =
        WebviewWindowBuilder::new(app, label.clone(), WebviewUrl::App("index.html".into()))
            .title(title)
            .maximized(true)
            .inner_size(1220.0, 860.0)
            .min_inner_size(960.0, 620.0);

    if let Some(path) = file_path {
        let serialized = serde_json::to_string(&path.to_string_lossy().to_string())
            .map_err(|err| format!("Failed to prepare window bootstrap script: {err}"))?;
        builder = builder
            .initialization_script(&format!("window.__TODOMARK_LAUNCH_FILE__ = {serialized};"));
    }

    builder
        .build()
        .map_err(|err| format!("Failed to create TodoMark window: {err}"))
}

fn open_or_focus_markdown_window(app: &AppHandle, markdown_path: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(markdown_path)
        .map_err(|err| format!("Failed to resolve markdown path: {err}"))?;

    let is_markdown = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false);
    if !is_markdown || !canonical.is_file() {
        return Err("Only existing .md files can be opened.".to_string());
    }

    let file_key = canonical_key(&canonical);

    let existing_label = {
        let registry = app.state::<WindowRegistry>();
        let file_to_window = registry
            .file_to_window
            .lock()
            .expect("window registry poisoned");
        file_to_window.get(&file_key).cloned()
    };

    if let Some(label) = existing_label {
        if let Some(window) = app.get_webview_window(&label) {
            focus_window(&window);
            return Ok(());
        }

        remove_window_registration(app, &label);
    }

    let label = next_window_label(app);
    let window = build_window(app, label.clone(), Some(&canonical))?;
    register_file_window(app, &file_key, &label);
    attach_destroy_cleanup(&window, label);
    focus_window(&window);

    Ok(())
}

fn focus_any_window(app: &AppHandle) -> bool {
    if let Some((_, window)) = app.webview_windows().into_iter().next() {
        focus_window(&window);
        return true;
    }

    false
}

fn setup_initial_windows(app: &AppHandle) {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().ok();
    let initial_paths = extract_markdown_paths(&args, cwd.as_deref());

    if initial_paths.is_empty() {
        return;
    }

    for path in initial_paths {
        let _ = open_or_focus_markdown_window(app, &path);
    }
}

fn handle_single_instance_event(app: &AppHandle, args: Vec<String>, cwd: String) {
    let cwd_path = if cwd.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(cwd))
    };

    let paths = extract_markdown_paths(&args, cwd_path.as_deref());
    if paths.is_empty() {
        let _ = focus_any_window(app);
        return;
    }

    for path in paths {
        let _ = open_or_focus_markdown_window(app, &path);
    }
}

fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    let open_new_item = MenuItem::with_id(
        app,
        OPEN_NEW_MENU_ID,
        "Open New...",
        true,
        Some("CmdOrCtrl+O"),
    )?;
    let mut has_file_submenu = false;

    for item in menu.items()? {
        let Some(submenu) = item.as_submenu().cloned() else {
            continue;
        };

        if submenu.text().map(|text| text == "File").unwrap_or(false) {
            submenu.prepend(&open_new_item)?;
            has_file_submenu = true;
            break;
        }
    }

    if !has_file_submenu {
        let file_submenu =
            Submenu::with_id_and_items(app, "file", "File", true, &[&open_new_item])?;
        menu.prepend(&file_submenu)?;
    }

    Ok(menu)
}

fn open_markdown_from_dialog(app: &AppHandle) {
    let app_handle = app.clone();

    app.dialog()
        .file()
        .set_title("Open Markdown File")
        .add_filter("Markdown", &["md"])
        .pick_file(move |selection| {
            let Some(file_path) = selection else {
                return;
            };

            let Ok(path) = file_path.into_path() else {
                return;
            };

            let _ = open_or_focus_markdown_window(&app_handle, &path);
        });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(WindowRegistry::default())
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            if event.id() == OPEN_NEW_MENU_ID {
                open_markdown_from_dialog(app);
            }
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            handle_single_instance_event(app, args, cwd);
        }))
        .setup(|app| {
            setup_initial_windows(&app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            read_markdown,
            write_markdown,
            stat_markdown
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        if let RunEvent::Opened { urls } = event {
            let mut paths = Vec::new();
            let mut seen = HashSet::new();

            for url in urls {
                let Ok(path) = url.to_file_path() else {
                    continue;
                };
                push_unique_markdown_path(&path, &mut seen, &mut paths);
            }

            for path in paths {
                let _ = open_or_focus_markdown_window(app_handle, &path);
            }
        }
    });
}
