#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod air;
mod commands;
// Engine spine is Unix-only until M7: `bossclaw-core` (bundled-SQLCipher + rustix) doesn't build
// on Windows yet, so the desktop ships without the engine there. M7 un-gates it.
#[cfg(unix)]
mod engine;
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
    let mut builder = tauri::Builder::default();

    // CF2: single-instance guard. POLICY_LOCK is in-process only, so two desktop instances could
    // each run a channel + AI loop and both reserve the same budget slot (cross-process race →
    // budget bypass). Register this FIRST (Tauri 2 requirement) so a second launch hands off to the
    // running process and exits; the callback focuses the existing main window.
    //
    // M-3: `#[cfg(desktop)]` here = Tauri's `not(any(android, ios))`, which matches the dependency's
    // `cfg(any(target_os = "macos", "windows", "linux"))` predicate in Cargo.toml — both resolve to
    // "desktop only" for every supported target, so the crate is always available where this compiles.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // One-time rename migration (BossClaw -> AIR Agent). Idempotent + best-effort;
            // safe on every launch. MUST run before the vault/identity store are used below.
            let vault = vault::default_vault();
            let legacy_vault = vault::vault_for_service(vault::LEGACY_IDENTITY_SERVICE);
            let _ = air::migrate_identity_keys(legacy_vault.as_ref(), vault.as_ref());
            vault::migrate_legacy_blob_once();

            let data_dir = app.path().app_data_dir().expect("app data dir");
            let _ = air::migrate_identity_metadata(&data_dir, vault::LEGACY_IDENTITY_SERVICE);

            // On Windows there is no engine, so `vault`/`data_dir` are not cloned (cloning then
            // dropping the clones would warn as unused) — move them straight into IdentityStore.
            #[cfg(unix)]
            let identity_store = IdentityStore::new(vault.clone(), data_dir.clone());
            #[cfg(not(unix))]
            let identity_store = IdentityStore::new(vault, data_dir);
            #[cfg(unix)]
            let engine = {
                let resource_dir = app.path().resource_dir().expect("resource dir");
                let model_dir = resource_dir.join("models/potion-base-8M");
                let provider = std::sync::Arc::new(crate::engine::embed::ResourceModel2Vec::new(model_dir));
                std::sync::Arc::new(crate::engine::EngineHandle::new(vault, data_dir, provider))
            };

            // Default to mock for dev; toggle to real AIR via AIR_AGENT_USE_REAL_AIR env var.
            // Settings UI will offer a friendlier toggle in a later task.
            let air_client: Arc<dyn air::AirClient> =
                if std::env::var("AIR_AGENT_USE_REAL_AIR").is_ok() {
                    Arc::new(HttpAirClient::production())
                } else {
                    Arc::new(MockAirClient::new())
                };

            app.manage(AppState {
                air_client,
                identity_store,
                inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
                #[cfg(unix)]
                engine,
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
            #[cfg(unix)]
            commands::engine::engine_status,
            a2a_demo_round_trip,
            commands::inbox::inbox_status,
            commands::inbox::inbox_identity,
            commands::inbox::inbox_start,
            commands::inbox::inbox_stop,
            commands::inbox::inbox_send,
            commands::inbox::inbox_conversations,
            commands::inbox::inbox_history,
            commands::inbox::inbox_policy_get,
            commands::inbox::inbox_policy_set,
            commands::inbox::inbox_ai_reserve,
            commands::inbox::inbox_ai_confirm,
            commands::inbox::inbox_ai_cancel,
            commands::inbox::inbox_default_agent,
            inbox::channel::inbox_channel_start,
            inbox::channel::inbox_channel_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
