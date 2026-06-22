# potion-base-8M (bundled embedding model)

The model files (model.safetensors, tokenizer.json, config.json) are NOT committed.
Run `scripts/fetch-model.sh` to download them (hash-pinned). This README is a
committed placeholder so the Tauri `bundle.resources` glob matches at least one
file in a fresh checkout — an empty glob is a hard `cargo check` error.
