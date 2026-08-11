mod cmd;
mod core;
mod error;
mod event;
mod protocol;
mod state;
mod window;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("dp-asset", protocol::asset_response)
        .register_uri_scheme_protocol("dp-template", protocol::template_response)
        .setup(|app| {
            let state = state::AppState::new(app.handle())
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            app.manage(state);
            // Windows 11 needs the rounded-corner preference set explicitly;
            // no-op elsewhere. See window.rs.
            if let Some(main) = app.get_webview_window("main") {
                window::round_corners(&main);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd::get_config,
            cmd::save_config,
            cmd::list_templates,
            cmd::import_asset,
            cmd::import_template_image,
            cmd::import_template_pack,
            cmd::import_document,
            cmd::import_document_asset,
            cmd::search_documents,
            cmd::create_job,
            cmd::get_job,
            cmd::list_jobs,
            cmd::cancel_job,
            cmd::delete_templates,
            cmd::save_asset,
            cmd::open_artifact
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
