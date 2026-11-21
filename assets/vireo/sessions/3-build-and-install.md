---
title: Build and install Vireo
description: Build Vireo from these sources, with your changes, and install it into Applications.
---
You are working in Vireo's own sources: Vireo is a code editor, a fork of Zed. Build Vireo from these sources and install it.

1. Check the prerequisites in docs/src/development/macos.md (Xcode, Rust, cmake). If something is missing, tell me exactly how to install it and stop.
2. Run `script/install-vireo-mac`. The first build takes 30 to 60 minutes; later ones are faster. Run nothing else with cargo while it builds.
3. If the build fails, show me the first error and what you think causes it, and propose a fix.
4. When it is installed, tell me to quit Vireo and open it again from /Applications.
