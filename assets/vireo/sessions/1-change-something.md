---
title: Change something
description: Describe a change to Vireo. Claude makes it in these sources, tests it and commits it.
input: What should change? For example: "make the tabs a little taller"
---
You are working in Vireo's own sources: Vireo is a code editor, a fork of Zed. I want this change:

{{input}}

How to work:
1. Read AGENTS.md and README.md first, and follow their rules.
2. Find the code involved, and tell me in a few lines what you plan before large changes.
3. Where the project has tests for that code, write a failing test first, then make the change.
4. Run the tests of the crates you touched (`cargo test -p <crate>`) and `./script/clippy -p <crate>`. Run one cargo command at a time: Vireo's builds use a lot of memory.
5. Commit with a clear message. Do not push unless I ask.
6. Tell me how to try it: `script/install-vireo-mac` builds Vireo from these sources and installs it into /Applications.
