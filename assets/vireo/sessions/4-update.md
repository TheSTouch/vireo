---
title: Update to the latest Vireo
description: Get the latest Vireo from GitHub, keep your own changes on top, then rebuild and install.
---
You are working in Vireo's own sources: Vireo is a code editor, a fork of Zed. Update these sources to the latest Vireo, then rebuild it.

1. Run `git status`. If there are uncommitted changes, show them to me and ask whether to commit them first. Never discard them.
2. Run `git pull --rebase` on the current branch. If the rebase stops on a conflict, show me the conflicting files and propose how to resolve each one before continuing.
3. Summarize what changed since my last update (`git log` over the new commits), in plain words.
4. Run `script/install-vireo-mac` to build and install the updated Vireo. Run nothing else with cargo while it builds.
5. When it is installed, tell me to quit Vireo and open it again from /Applications.
