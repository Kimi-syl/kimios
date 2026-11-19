# AI Studio + desktop pet research (P5)

**Status: RESEARCH.** PocketLLM (`/root/pocketllm`, `Kimi-syl/PocketLLM`) is a full Kotlin/Compose Android app doing local GGUF inference via **llama.cpp over JNI** (Vulkan optional, two CPU kernel variants), with an OpenAI-compatible **Ktor server** (`/v1/chat/completions` SSE, `/v1/models`), HuggingFace resumable segmented downloader, **on-device image gen** (vendored stable-diffusion.cpp), TTS (Android + Piper/sherpa-onnx), agent mode — and **already a complete desktop-pet "companion" subsystem** (`companion/`: `CompanionOverlayService`, `CompanionBrain`, `Live2DOverlay` using Cubism SDK for Java + 8 sample models, `VrmCharacter` using Filament). Models on this machine: LFM2.5-230M-Q4_K_M (153 MB), Qwen3.8-2B-Q4_K_M (1.31 GB).

## AI Studio integration — recommendation: guest `llama-server`
KimiOS can't run PocketLLM as an APK (APK compat abandoned). Options assessed:
- (a) launch PocketLLM via intent — UI lives outside KimiOS framebuffer (breaks one-OS invariant) → escape hatch only.
- (b) embed llama.cpp JNI into libkimios.so — duplicates PocketLLM plumbing → Phase 3.
- (c) KimiOS client of PocketLLM Ktor — Chat works, but no model/download endpoints → optional secondary backend.
- **(d) `llama-server` as a native guest binary — RECOMMENDED.** Static-musl aarch64 build of llama.cpp's server (OpenAI API + SSE + router mode `--models-dir`/`--models-max` LRU + `--api-key`). Self-contained inside KimiOS (own models dir/downloads/settings), reuses upstream wholesale, isolates inference in a sacrificial child. Launch via `SYS_RUN_PATH` in a `:guest` service process (phantom-killer mitigation). Rust GUI = small no_std JSON/SSE client over `SYS_NET`.

Features: **Chat** (SSE streaming, 2B Q4 ctx2048 4 threads — realistic, PocketLLM runs these here), **Models** (`/disk/models` listing + GGUF header inspect + load via router), **Downloads** (guest-Python resumable HF downloader — NOT `SYS_FETCH_URL` which blocks GUI/no resume; HF search via fetch_url), **Settings** (JSON in /disk → server argv), **ImageGen** (v1 remote API or PocketLLM handoff; v2 stable-diffusion.cpp guest CLI — CPU-heavy minutes/image, don't colocate with loaded LLM on 4 GB).

## Desktop pet — recommendation: Live2D (VRM later)
- **Live2D wins**: 2D textured meshes fit the software framebuffer (~300×500 px ≈ 0.6 MB/frame readback, same order as the shipped WebView blit). VRM/Filament is full PBR 3D — PocketLLM's own code documents fragility (256-joint hard limit, 2048² textures black-screen the GPU). The whole pet already exists in PocketLLM and is portable.
- **Rendering**: reuse the proven `SYS_WEB` pattern — `KimiPet.java` ports Cubism SDK for Java + `LAppMinimumDelegate` to an **offscreen EGL pbuffer** (or VirtualDisplay+Presentation+ImageReader), `glReadPixels` RGBA → direct ByteBuffer → seqlock; new `SYS_PET=53` multiplexed (init/render/load/motion/expression/setParam/touch/destroy). Rust alpha-blits the pet **last** (always-on-top layer) into the XRGB back buffer; drag/tap/long-press handled in Rust; bubble text via the Rust font stack. Idle 15–30 fps, dirty-gated, pause when covered.
- **Brain**: port `CompanionBrain` (local-LLM-first + cloud fallback) → POST to the same llama-server (230M model for pet, 2B for chat; `-np 2` slots). **Voice**: `SYS_TTS` (Android TextToSpeech) + `UtteranceProgressListener` → mouth-sync `ParamMouthOpenY` (PocketLLM's exact approach).

## Licensing (decisive)
- **Cubism SDK**: free for individuals/small enterprises (<¥10M sales) **EXCEPT "Expandable Applications"** (user-imported models → contract + revenue share, free apps usually ineligible). **→ ship FIXED bundled models, NO .moc3 import.**
- **Sample models**: Hiyori (no design changes), Haru, Mao, Rice, Ren, Mark, Wanko usable with copyright notice ("uses sample data owned by Live2D Inc." in Settings→About). **Drop Natori** (collaboration char: no commercial/redistribution). Don't republish SDK/models standalone.
- **VRM** (v2 option): open glTF-based, per-avatar license embedded in file meta. `/root/Seed-san.vrm` (VRM 1.0, VirtualCast) declares everyone/corporation/allow-redistribution → safe to bundle/import. Render via Filament+readback (WebGL blank in Phase-1 software draw).

## Risks
RAM (2B ≈ 1.6–2 GB total; ctx2048, models-max 1, mmap, pet on 230M, never SD+LLM together on 4 GB); phantom-process killer (`:guest` service); 4 KiB page assumption; CPU-only guest (Vulkan unproven in forked musl child); `SYS_FETCH_URL` blocks GUI (guest downloader mandatory); Live2D expandable-app trap (no import); readback memcpy cost (pet ≤300×500, 15–30 fps).

Sources: pocketllm skill + `/root/pocketllm/{README,THIRD_PARTY_NOTICES,server/ApiServer,models/ModelRepository,companion/*,com/live2d/demo/minimum}`; `/root/l2d_assets/CubismSdkForJava-5-r.5`; live2d.com SDK/license/expandable/sample-model-terms; llama.cpp server README; vrm.dev; Filament. Full plan in the P5 research task report.
