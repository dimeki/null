ytmusic-lite
============

A small music player for YouTube Music that stays light on memory.

Starting it
-----------
Run ytmusic-lite.exe. Search for a song and click it, or sign in with Google in the
Library tab to use your playlists. Closing the window keeps the music playing from the
tray icon (bottom right, possibly under the "^" arrow). Right-click the tray icon and
choose Quit to close it completely.

Keep the files together
-----------------------
ytmusic-lite.exe needs the other files in this folder (libmpv-2.dll, yt-dlp.exe,
deno.exe). If you moved it, move the whole folder.

Where your data lives
---------------------
%LOCALAPPDATA%\ytmusic-lite holds your queue, settings, downloaded songs and, if you
signed in, your sign-in cookies. Delete that folder to start fresh. The sign-in cookies
are not encrypted, so treat the folder like a saved password.

Updates
-------
The app checks once a day for a newer version and shows a notice in Settings. You can
turn that off there. Songs are fetched with yt-dlp, which YouTube breaks from time to
time: if songs stop playing, press Update next to yt-dlp in Settings.

Listening parties
-----------------
Open the Party tab. "Start a party" gives you a code to share; "Join a party" takes
someone else's code. Everyone streams the music themselves, so nobody's audio is sent
through anyone else.

Third-party software
--------------------
See THIRD-PARTY-NOTICES.txt.
