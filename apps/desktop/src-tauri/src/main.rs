#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod air;
mod commands;
mod file_access;
mod inbox;
mod llm_stream;
mod markitdown;
mod secrets;
mod skills;
mod vault;
mod web_access;

use crate::commands::a2a::a2a_demo_round_trip;

use crate::air::{HttpAirClient, IdentityStore, MockAirClient};
use crate::commands::identity::*;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let vault = vault::default_vault();
            let data_dir = app.path().app_data_dir().expect("app data dir");
            let identity_store = IdentityStore::new(vault, data_dir);

            // Default to mock for dev; toggle to real AIR via BOSSCLAW_USE_REAL_AIR env var.
            // Settings UI will offer a friendlier toggle in a later task.
            let air_client: Arc<dyn air::AirClient> =
                if std::env::var("BOSSCLAW_USE_REAL_AIR").is_ok() {
                    Arc::new(HttpAirClient::production())
                } else {
                    Arc::new(MockAirClient::new())
                };

            app.manage(AppState {
                air_client,
                identity_store,
                inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            markitdown::md_detect,
            markitdown::md_install,
            markitdown::md_convert,
            skills::skills_list_verified,
            skills::skills_install_verified,
            file_access::file_exists,
            file_access::file_read,
            file_access::file_write,
            web_access::web_auth_set,
            web_access::web_auth_has,
            web_access::web_auth_delete,
            web_access::web_fetch_public,
            web_access::web_fetch_auth,
            web_access::pw_detect,
            web_access::pw_install,
            web_access::pw_fetch_rendered,
            llm_stream::llm_plan,
            llm_stream::llm_list_models,
            llm_stream::llm_openai_compat_list_models,
            llm_stream::gemini_generate,
            llm_stream::claude_generate,
            llm_stream::llm_stream_start,
            llm_stream::llm_stream_cancel,
            is_onboarded,
            get_identity,
            get_trust_score,
            create_identity,
            reset_identity,
            a2a_demo_round_trip
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
