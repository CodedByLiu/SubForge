<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./public/logo-2.png">
    <img src="./public/logo-1.png" alt="SubForge logo" width="180">
  </picture>
</p>

<h1 align="center">SubForge</h1>

<p align="center">
  A desktop app for local subtitle extraction, segmentation, and translation.
</p>

<p align="center">
  <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="React 19" src="https://img.shields.io/badge/React-19-149ECA?logo=react&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white">
  <img alt="Windows first" src="https://img.shields.io/badge/Platform-Windows%20first-0078D4">
</p>

![SubForge screenshot](./docs/images/subforge.png)

## What is SubForge?

SubForge is a Tauri desktop app for turning local videos into `.srt` subtitles with a practical batch workflow. It combines local `whisper.cpp` transcription, optional VAD-based preprocessing, subtitle segmentation, and optional translation through OpenAI-compatible APIs.

The current implementation is Windows-first and portable by design: runtime files, downloaded models, config, logs, and task cache are stored next to the application.

## Highlights

- Local subtitle extraction with `whisper.cpp`
- Batch video import and task queue management
- Whisper model download and management inside the app
- GPU detection and optional GPU inference
- Optional VAD to avoid bad timing on intros, music, and long silence
- Configurable subtitle segmentation strategies
- Translation can be fully disabled, routed through an OpenAI-compatible LLM, or switched to an experimental Google Web mode
- Three output modes:
  - original subtitles only
  - original and translated subtitles as two files
  - bilingual subtitles in a single `.srt`
- Portable storage layout with encrypted API key persistence

## Screenshot

The current main window focuses on a simple batch workflow: choose the output directory, import videos, start or pause tasks, and track original/translated subtitle progress in one table.

## Quick Start

### Clone

```bash
git clone https://github.com/CodedByLiu/SubForge.git
cd SubForge
```

### Prerequisites

- Node.js 20+
- Rust stable toolchain
- Windows build environment for Tauri 2
- `ffmpeg` available in `PATH`, or set explicitly in Settings

Notes:

- If `whisper-cli` is not configured, SubForge can bootstrap a managed `whisper.cpp` runtime on Windows.
- Whisper model weights are downloaded from inside the app.
- When VAD is enabled, the app also prepares the required VAD model automatically.

### Development

```bash
npm install
npm run tauri dev
```

### Production Build

```bash
npm install
npm run tauri build
```

## How it Works

1. Import one or more local videos.
2. Choose whether subtitles should be written next to each video or into one shared output directory.
3. Configure transcription, segmentation, translation, and runtime limits in Settings.
4. Start the queue.
5. SubForge runs the pipeline:

```text
video -> ffmpeg -> VAD (optional) -> whisper.cpp -> segmentation -> translation (optional) -> .srt output
```

## Project Structure

```text
src/                 React UI
src-tauri/           Rust backend, Tauri shell, task pipeline
docs/images/         README assets
public/              app logo assets
specs/               product and implementation notes
```

## Portable Runtime Layout

At runtime, SubForge creates and uses directories like:

```text
<app-dir>/
  bin/
  config/
  data/
  logs/
  models/whisper/
  temp/
```

This keeps the app self-contained and easy to move between machines or folders.

## Current Feature Set

- Task import, start, pause, resume, delete, and clear
- Task snapshots so running jobs keep their own config
- Hardware probing for CPU, memory, GPU, and recommended Whisper model tiers
- Whisper model catalog: `tiny`, `base`, `small`, `medium`, `large-v3`
- Segmentation strategies:
  - `disabled`
  - `auto`
  - `rules_only`
  - `llm_preferred`
- Translation engines:
  - `none`
  - `llm`
  - `google_web` (experimental)

## Roadmap

- Better packaging and release flow
- More validation and recovery around transcription dependencies
- More polished task diagnostics and error reporting
- Continued refinement of segmentation and translation quality

## Notes

- There is no license file in this repository yet.
- The managed `whisper.cpp` bootstrap currently targets Windows.
- Experimental Google Web translation may break when upstream behavior changes.

