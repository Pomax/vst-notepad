# A vibe-coded VST3 Notepad effect

You compose and produce: have notes on your projects. You're probably keeping them in text files in some folder, maybe even managed using Obsidian or something. But let's be honest: why?

Not the notes part, that part makes perfect sense, but why would your notes need to live separately from your actual project?

## "My DAW doesn't let me s-"

No, no, no, I'm not talking about a project setting, I'm talking about literally adding your notes to your project. Using a notepad effect on your master.

Think FL Studio's "Fruity Notepad" or Melda Production's MNotepad.

### "Yeah but those are... kind meh? I don't use FL Studio, and MNotepad is small and crowded?"

True. They also don't support markdown, and let's be real. Notes in plain text? Seriously? In `{insert year here}`? Let's write our notes in Markdown instead, with inline editing (i.e. you see markdown if your cursor is on something with markdown syntax, and just "the text" if it's not).

Hell let's just Star Trek that: "Computer, make me a VST3 note-taking effect plugin that supports markdown".

# Installing the plugin

## Downloads are [here]()

As this is a VST3 release, there's only Windows and a MacOS releases, because Linux does not support VST3. If you're on Linux, have a look at https://github.com/robbert-vdh/yabridge, which might work for you.

## Installation is a file copy

VST3, unlike VST/VST2, has only one top level location it's allowed to go in, to prevent the absolutely bullshit that VST2 had with "everything drops plugins in different dirs good luck lol".

### Windows

Put the vst3 file in `C:\Program Files\Common\VST3` (either top level, or you can put it in its own folder inside of that), then tell your DAW to rescan for plugins.

### MacOS

Put the vst3 "file" in `/Library/Audio/Plug-Ins/VST3/` (either top level, or you can put it in its own folder inside of that), then tell your DAW to rescan for plugins.

Note that MacOS technically has two locations, because unlike Windows it has a proper "system vs user" file system. So `~/Library/Audio/Plug-Ins/VST3/` also works. Same dir, just the `user` version (i.e. just for you) rather than the `system` version (i.e. for all users).

# Manually building the plugin

Tell Claude to run the platform-appropriate build script, or run it yourself, of course.
