---
title: Fix a bug
description: Describe what goes wrong. Claude finds the cause, adds a test that shows it, and fixes it.
input: What goes wrong, and how do you make it happen?
---
You are working in Vireo's own sources: Vireo is a code editor, a fork of Zed. I found this bug:

{{input}}

How to work:
1. Read AGENTS.md and README.md first, and follow their rules.
2. Find the cause before changing anything, and tell me what it is.
3. Write a test that fails because of the bug, then fix the code so it passes.
4. Run the tests of the crates you touched (`cargo test -p <crate>`) and `./script/clippy -p <crate>`. Run one cargo command at a time: Vireo's builds use a lot of memory.
5. Commit with a message that says what was wrong and why. Do not push unless I ask.
6. Tell me how to try it: `script/install-vireo-mac` builds Vireo from these sources and installs it into /Applications.
