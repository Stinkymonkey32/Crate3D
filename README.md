# Crate

**Crate** is an independent game platform and runtime capable of running Roblox experiences through its own engine.

Crate is designed to take existing Roblox place data and run it through a completely independent runtime, without requiring games to be manually ported to another engine.

> **Make a game once. Run it with Crate.**

## ✨ Features

* 🎮 Run supported Roblox experiences
* ⚡ One-click game launching
* 🧱 Custom 3D runtime
* 🌎 Custom physics
* 👤 Player movement and camera
* 📦 Roblox place/object compatibility
* 🔌 Networking
* 📜 Scripting support
* 🚧 Standalone editor in development

## How it works

Currently, Crate uses Roblox Studio as part of its import pipeline.

```text
Roblox Studio
      │
      ▼
   .rblxlx
      │
      ▼
    Crate
      │
      ▼
 Running Experience
```

The long-term goal is to remove the Roblox Studio dependency and provide a completely standalone development workflow through Crate Studio.

## Crate Studio

Crate Studio is the planned standalone editor for creating and editing experiences directly for Crate.

The goal is to eventually provide:

* 3D scene editing
* Explorer / hierarchy
* Properties editor
* Transform tools
* Asset management
* Scripting
* Project management
* Play / test mode

## Compatibility

Crate is still under active development.

Not every Roblox feature is currently supported, and compatibility will continue to improve over time.

Crate does not contain Roblox source code and is implemented independently.

## Project Structure

The main engine repository is currently named **Crate3D**.

The public-facing project and platform are simply called **Crate**.

## License

Crate is protected under the **PolyForm Perimeter License**.

See [`LICENSE`](LICENSE) for the complete license terms.

## Status

🚧 **Early development**

Crate is experimental software. APIs, formats, compatibility, and other parts of the project may change significantly during development.

## Disclaimer

Crate is an independent project and is not affiliated with, sponsored by, or endorsed by Roblox Corporation.

#
Required Notice: Copyright Stinkymonkey32 (https://github.com/stinkymonkey32
