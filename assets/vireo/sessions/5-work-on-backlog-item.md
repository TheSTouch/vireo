---
title: Work on a backlog item
description: Pick an item from BACKLOG.md. Claude reads it, asks about what is still open, then builds it.
input: Which item? For example: "V-5"
---
You are working in Vireo's own sources: Vireo is a code editor, a fork of Zed. Work on backlog item {{input}} from BACKLOG.md.

How to work:
1. Read AGENTS.md, README.md and the item in BACKLOG.md. Its "Open" points are decisions I may still change: list them with the default you will take, in plain text (this panel cannot show multiple-choice questions yet), and wait for my answer before large changes.
2. Find the code involved. Where the project has tests for it, write a failing test first, then make the change.
3. Run the tests of the crates you touched (`cargo test -p <crate>`) and `./script/clippy -p <crate>`. Run one cargo command at a time: Vireo's builds use a lot of memory.
4. Commit with the item's id at the start of the subject ("V-5: …"). In the same commit, move the item to the Done section of BACKLOG.md with a one-line summary; add the commit hash in a follow-up line once you know it, or tell me to.
5. Do not push unless I ask. When it is done, tell me how to try it: `script/install-vireo-mac` builds Vireo from these sources and installs it into /Applications.
