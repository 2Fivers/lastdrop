# Lastdrop user guide

How to install Lastdrop, read what it shows and set it up for your services. For how the app is
built, see [ARCHITECTURE](../ARCHITECTURE.md) and the [design specs](../design) instead.

1. [Getting started](getting-started.md): install, first run, the tray icon, the taskbar readout,
   the flyout and the hover card, start with Windows, updates and uninstalling.
2. [Reading the flyout](reading-the-flyout.md): meters, reset times, pace, burn rate and forecast,
   the usage history window, and the notifications.
3. [Accounts](accounts.md): several Claude Code or Codex accounts, folders found on your PC, and
   hiding e-mail addresses.
4. [Adding your own service](custom-services.md): describe a service with a simple usage or balance
   API in a JSON file.
5. [What Lastdrop reads](what-lastdrop-reads.md): for each service, which file, program, web
   address and key it uses, and how often.
6. [Troubleshooting](troubleshooting.md): errors and "sign in again", where Lastdrop keeps its
   files, starting over, and the language.

The guide follows the code in this repository, which can be ahead of the latest release. In
Lastdrop 0.1.1 and earlier the settings window is one long page with an **Apply** button instead
of four pages that save at once, and the flyout's footer has no usage history button (use *Usage
history…* in the tray menu). There, accounts and API keys have headings of their own,
**Accounts** and **API keys**, instead of opening from their service's row under **Services**;
the settings in them have the same names.
