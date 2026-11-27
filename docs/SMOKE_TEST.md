# Desktop smoke test

A short check by hand on Windows, for changes the automated tests cannot reach: Explorer, the tray, the right-click menu. It takes about ten minutes. Use files you make for it, never your own.

**Setup.** Run the app from source (`npm run tauri -- dev`) or install a build. Make a folder such as `C:\zc-smoke`, add it as an always-on folder in Settings, and put in it a few small files: two PNG pictures, one WAV, and a folder `pics` with three PNGs.

Each step says what you should see. If something else happens, stop and note the step, what you did and what you saw.

## Queue

1. Drop a PNG into the window, choose WebP, convert. The WebP appears next to the PNG, which stays.
2. Convert the same PNG again. The new file is named `name (1).webp`; the first one is untouched.

## Rename to convert

3. In Explorer, rename `a.png` to `a.webp`. A notice says it was converted; `a.webp` is a real WebP and `a.png` is in the Recycle Bin.
4. Rename `b.png` to `b.png@64`. The result is `b.png`, at most 64 pixels on its longer side; the original is in the Recycle Bin.
5. Rename `c.png` to `c.jpg&`. `c.jpg` appears and `c.png` stays.
6. Rename `d.png` to `d.webp+`. A small window shows the WebP options (Compression, and Quality once Lossy is chosen) and the longest side, with Cancel and Convert in view. It is in the taskbar and in Alt+Tab, titled `d.webp · ZeroConverter`. Close it with its X. The file is named `d.png` again.
7. Rename the folder `pics` to `pics.webp`. Its three files become WebP; the folder keeps its name.
8. On the Desktop, convert a file by renaming it. The new file's icon stays where the old one was.
9. Turn on **Keep the original file** in Settings; the `&` line under **Add to the new name** now says it sends the original to the Recycle Bin. Rename a PNG to `.webp`: both files stay, and the notice says Original kept. Rename another PNG to `.webp&`: only the `.webp` stays, the original is in the Recycle Bin, and the notice says Original is in the Recycle Bin. Turn **Keep the original file** back off.

## Right-click menu

10. Right-click a PNG: **Convert with ZeroConverter → To WEBP**. A copy appears next to it; the original stays.
11. Turn rename-to-convert off in Settings, then use the menu again. It still works. Rename a file to `.webp`: nothing happens.
12. Turn the right-click menu off in Settings. The menu disappears from Explorer (it may take a moment).

## Stopping and closing

13. Turn rename-to-convert on, rename a large file, and quit from the tray at once. The app closes within a few seconds; the file is either converted or back under its old name, never missing.
14. Start the app again. No `.zc-*` or `zero_tmp_*` folders are left next to the files you converted.

## Uninstall (installed builds only)

15. Uninstall. The right-click menu is gone from Explorer, and the files you converted are still there.
