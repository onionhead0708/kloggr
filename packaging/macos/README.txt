kloggr for macOS (11 or later, Apple Silicon and Intel)
=======================================================

1. Drag kloggr into the Applications folder (in this window).
2. Before the first start, allow kloggr to run (below).

kloggr is not signed with an Apple Developer ID or notarized, so macOS
blocks it the first time with a message like "kloggr" can't be opened,
"Apple could not verify kloggr is free of malware", or "kloggr is damaged
and can't be opened". kloggr is not damaged: macOS says this about every
downloaded app that isn't notarized.


Allow kloggr with a command (quickest)
--------------------------------------

Open Terminal (Applications > Utilities > Terminal), paste this line and
press Return:

    xattr -dr com.apple.quarantine /Applications/kloggr.app

Then open kloggr as usual. This removes the "downloaded from the internet"
mark that makes macOS block the app. Run it again after installing a new
version.


Allow kloggr without Terminal
-----------------------------

1. Open kloggr from Applications (double-click). macOS shows a message that
   it can't be opened: click Done (or OK). Don't move it to the Bin.
2. Open the Apple menu > System Settings > Privacy & Security.
3. Scroll down to the Security section. Next to "kloggr" was blocked to
   protect your Mac (or "kloggr" was blocked from use because it is not
   from an identified developer), click Open Anyway.
4. Confirm with Open Anyway, and enter your password or use Touch ID if
   asked.
5. kloggr starts. From now on it opens normally.

The Open Anyway button appears only for about an hour after step 1: if it
isn't there, open kloggr again and go back to Privacy & Security.

If macOS says kloggr "is damaged", there is no Open Anyway button: use the
Terminal command above.

On macOS 14 (Sonoma) and older, right-clicking (or Control-clicking) kloggr
in Applications, choosing Open and then Open in the message also works.


More
----

Source, documentation and issues: https://github.com/onionhead0708/kloggr
License: GPL-3.0-or-later (LICENSE.txt)
