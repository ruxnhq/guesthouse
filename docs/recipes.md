# Recipes

A recipe says what a machine is for. In New Machine, the row *Set up for* lists them, and *Nothing extra* is first and is what you get unless you pick one, so a machine made without a recipe is made exactly as it always was. A recipe can set a few of the machine's settings when it's made, and once the install has finished and the guest is up, it runs its steps in the guest, once.

I haven't run a recipe's steps in a real guest yet. The tests use a pretend guest, and what each needs from a real one is at the end of this page.

## Where recipes come from

- **Comes with Guesthouse.** One file each, in the program folder's `recipes/`, or the repo's.
- **Your own.** A file each in `~/.config/guesthouse/recipes/`, which is yours to make. A file there that isn't a recipe is told to you with its name and the reason, and the rest still show.

Both are a text file ending in `.toml`. The dialog says which a recipe is. A recipe of your own runs its script as the guest's administrator, which is your call to make, and nobody else has read it. So when you pick one that has steps, the dialog shows you all of it, the script as it is, and asks before it makes the machine. The recipes that come with the app are one click away under *Show*.

What you agreed to is kept in the machine's folder (`recipe.json`, and how its steps went in `recipe-result.json`), as the machine's own copy. Running the steps again runs that copy, so a file you changed since is never run in a machine you haven't seen it for. To use the changed file, make a new machine.

## The file

```toml
name = "Games"
description = "Windows with room for games, and Steam."
guest = "windows"

[settings]
cores = 8
ram_gib = 16
disk_gib = 256

[steps]
packages = ["Valve.Steam"]
script = '''
Write-Output "Anything PowerShell can run."
'''
```

- `name` is at most 40 characters and `description` at most 160, each one line of plain text.
- `guest` is `windows` or `ubuntu`. A recipe shows only for a disc of that kind. A plain VM, a Linux you installed yourself and macOS have no recipes, as there's no way in for the steps.
- `[settings]` is optional, and so is each setting in it: `cores`, `ram_gib` and `disk_gib`, and for a Windows recipe `vram_gib`, the cap on video memory, where 0 is no cap. They are the same settings the dialog and `guesthouse-vm set` have, and they're held to the same limits. What this PC can't give comes down to what it can, and the dialog says so. Picking a recipe puts its sizes in the rows, and you can change them after. On the command line, what you type wins over the recipe.
- `lean = true` turns on [lean Windows](#lean-windows) for the machine, in a Windows recipe only. The person making the machine can still turn it off.
- `scan_exclusions` is a list of folders, such as a games library, that Defender's real-time scanning may [leave out](#leaving-folders-out-of-virus-scanning), in a Windows recipe only. The recipe only names them. Whoever makes the machine turns it on, and it's off unless they do.
- `[steps]` is optional too. `packages` is a list of names, each its own step, and `script` runs after them. On Windows the packages are winget ids and the script is PowerShell. On Ubuntu they are apt packages and the script is run by bash.
- A name can only be letters, digits and `. _ + -`, and never starts with a hyphen. On Ubuntu, only small letters. At most 30 packages, and the script is at most 32 KiB. A file that says anything else, a word the format doesn't have, or a name that isn't a name, is refused with the line it's on.

## How the steps run

They run as a stage of their own, after the install has fully succeeded and the machine has started. A step that fails never turns an install that worked into a failed machine: the machine stays, you're told which step stopped and what the guest said, and the machine's settings have the recipe with *Show* and *Run Again*. Steps run in order, and the first that fails stops the rest. Running again starts from the top, and a package that's already there is skipped.

If the PC has no room to start the machine after its install, it stays off with its steps waiting, and they run at its first start, with the same progress, once. A start that ends in the middle of them doesn't run them again by itself: the machine's settings say they were cut short, and *Run Again* is yours to press.

How the steps went is kept with the machine's other notes about its guest. A snapshot keeps it, so going back to one from before the steps ran says they haven't. A throwaway loses what they install when it shuts down, so an install there, as Start Over does, leaves its steps waiting and says why. Run Again on a throwaway runs them for that run only, and its next start says they haven't run.

Nothing from a recipe is ever put into a script as text. On **Windows**, the app sends the recipe's steps as Base64 JSON to a script of its own, which reads them as data: a package id is one argument to winget, and the script is written to a file and run from there. The agent in the guest runs as SYSTEM and takes one request at a time, and winget belongs to a user, so a short request starts the steps in a task of their own as the guest's account, with administrator rights. The app then reads a status file with quick requests, so a step that takes twenty minutes holds nothing up. On **Ubuntu**, the app runs `ssh` as root on the port forwarded to the guest, with the machine's own key, and ignoring your own SSH configuration. The command is always a fixed text. The runner (`linux/recipe/run.sh`) and the list of steps go in on stdin as files, so a name or script of a recipe is never read by a shell as anything but a name or a file to run. The guest counts as up once its first boot has finished and it has restarted, which can take a while after an install, and once that boot's GPU step has ended, with the GPU on or off, so a step that installs GPU software such as CUDA finds it done. A guest with no GPU step doesn't wait for one.

The Windows steps wait up to ten minutes for winget, as a new Windows 11 can take a while to register it. If it doesn't come, the step says so and the machine's settings can run the steps again later.

## Lean Windows

A choice of its own for any Windows machine, with or without a recipe: the *Lean Windows* row in New Machine, `--lean on` on the command line, or `lean = true` in a recipe. It's off unless one of those turns it on, and a machine made with it off is made exactly as before, to the byte of its answer file and every script. *Show* in the row lists everything below, as it will be done.

Nothing is changed in your ISO. When it's on, the answer disc carries `guest/lean.ps1` and the list as data, and the answer file gets one more command in Setup's *specialize* pass, the last moment before any account exists. There the script takes the apps out of what Windows installs for each new account, so the account never gets them, and sets the policies. Once the install is up, before its shutdown, the app runs the same script again through the agent, as SYSTEM, for every account, so that anything that reached one anyway goes too. That's a step of the install with its own name, and if any of it doesn't happen the machine is installed all the same, with a note saying what. The machine's settings have *Run Again*, and `guesthouse-vm lean NAME` does the same.

It only does two kinds of thing, both documented and both undone the documented way:

- It removes these app packages, by their package name, which is the same in every language and edition, compared whole and never as a pattern. One that isn't there is skipped, so Windows 11 IoT Enterprise LTSC, which has few of them, is fine. Each comes back from the Store.
  - Phone Link (`Microsoft.YourPhone`), Teams (`MSTeams`, and `MicrosoftTeams` on the first Windows 11 releases), Cortana (`Microsoft.549981C3F5F10`): a program that runs in the background from logon.
  - Copilot (`Microsoft.Copilot`): a program that can start at logon, and its updates.
  - Tips (`Microsoft.Getstarted`): the tips it shows after the first logon.
  - Maps (`Microsoft.WindowsMaps`): disk space, and its map updates.
  - News (`Microsoft.BingNews`), Weather (`Microsoft.BingWeather`), Microsoft 365 (`Microsoft.MicrosoftOfficeHub`), Outlook (`Microsoft.OutlookForWindows`), Mail and Calendar (`microsoft.windowscommunicationsapps`), Solitaire Collection (`Microsoft.MicrosoftSolitaireCollection`), Clipchamp (`Clipchamp.Clipchamp`), Power Automate (`Microsoft.PowerAutomateDesktop`), To Do (`Microsoft.Todos`), Dev Home (`Microsoft.Windows.DevHome`), Feedback Hub (`Microsoft.WindowsFeedbackHub`), Movies & TV (`Microsoft.ZuneVideo`), Family (`MicrosoftCorporationII.MicrosoftFamily`), and on Windows 10 People (`Microsoft.People`) and Skype (`Microsoft.SkypeApp`): disk space, and the Store keeping each up to date.
- It sets these policies, each a number under `HKLM\SOFTWARE\Policies`. Deleting the value undoes it.
  - `Microsoft\Windows\CloudContent` `DisableWindowsConsumerFeatures = 1`: no suggested apps from other makers in the new account. Enterprise, Education and IoT Enterprise honour it, and Home and Pro ignore it.
  - `Microsoft\Dsh` `AllowNewsAndInterests = 0`: Widgets are off, so their programs don't run from logon.
  - `Microsoft\Windows\OneDrive` `DisableFileSyncNGSC = 1`: OneDrive doesn't run or sync.
  - `Microsoft\Edge` `StartupBoostEnabled = 0` and `BackgroundModeEnabled = 0`: Edge doesn't start at logon, and doesn't keep running after its last window closes.

Windows then doesn't add two packages it otherwise adds after the first logon, which lean Windows never touches itself: `Microsoft.StartExperiencesApp`, Start's companion content such as the Phone Link panel, which seems to stay away with Phone Link, and `Microsoft.WidgetsPlatformRuntime`, the runtime Widgets run on, which seems to stay away with Widgets off. A plain and a lean install of the same Windows differed by these two besides the list.

Settings and Edge then say some settings are managed by your organization, as they do for any policy.

It leaves alone, on purpose: Defender, the firewall, SmartScreen, Windows Update, UAC, Secure Boot, the TPM and BitLocker, as Windows sets them; the Store, App Installer and winget, which recipes install with; every Xbox part, Game Bar and the Xbox identity provider, which games and Game Pass need; Edge and the WebView2 runtime; Get Help, which is Windows 11's troubleshooters, and Quick Assist; Photos, Media Player, Notepad, Paint, Calculator, Terminal, Snipping Tool, Camera, Clock, Sound Recorder and Sticky Notes; the runtimes, languages, input methods and accessibility tools; every service and scheduled task. The services that cost something at idle are the search indexer, SysMain, Defender and Update, and each is either kept on purpose or what Start's search needs.

How much faster or smaller it makes a guest hasn't been measured yet. The list says what each item saves in kind, not how much.

## Leaving folders out of virus scanning

This one makes a guest less safe, so it is a question of its own, off unless you say yes, and never turned on by a recipe or by any other choice. A recipe can only name the folders, with `scan_exclusions`. New Machine then has a row *Leave Folders Out of Virus Scanning*, which is off, and `guesthouse-vm new` takes `--skip-scanning on`. With a recipe that names no folders there's no row, so a machine made to try unknown things in, like the Clean box, is never offered it.

What it means, as the row says: Defender, Windows' own virus protection, doesn't check files in those folders as they're written or run. Defender stays on for everything else, and Windows Security shows it as on. It suits a machine for games or builds you trust, where Defender checking every file is much of what makes loading and installing slow, and not one for files or programs you don't know.

Near the end of the install the app gives each folder to Defender's own setting, `Add-MpPreference -ExclusionPath`, through the agent, with the folders as data. It's kept in the machine's folder (`scan-exclusions.json`), and the machine's settings and `guesthouse-vm show` say which folders Defender leaves out for as long as it does, or that it didn't take them, in which case every folder is scanned. *Scan Them Again* in the settings, or `guesthouse-vm scan-all NAME`, takes them back with `Remove-MpPreference`, and in the guest you can do the same yourself as administrator, or under Windows Security's exclusions. A folder is a full path such as `D:\Games`, of plain characters, at most 8 of them, with no name that starts or ends with a dot or a space, and never the whole of Windows, Program Files, ProgramData, Users or a profile, nor a Downloads, Desktop, Documents, AppData or Temp folder anywhere.

Why not turn real-time protection off for the whole machine: Windows doesn't let a program do that. Tamper Protection, which is on by default in every Windows nobody manages, blocks it, and a policy or a setting from a script only seems to work. Only a person in Windows Security can turn Tamper Protection off, and even then real-time protection turns itself back on after a while. Folder exclusions are Defender's own setting. Microsoft's pages say Tamper Protection guards them only on a device an organization manages, which a guest isn't, so they should work with it on. A real guest hasn't shown that yet.

## On the command line

```
guesthouse-vm recipes
guesthouse-vm new NAME --iso PATH --set-up-for RECIPE [--yes] [--lean on|off] [--skip-scanning on|off]
guesthouse-vm recipe NAME [--no-wait]
guesthouse-vm lean NAME [--no-wait]
guesthouse-vm scan-all NAME [--no-wait]
```

`recipes` lists them. `RECIPE` is a recipe's key (`app/games` or `yours/mine`), its file's name or its name, and a key is needed only if two fit. `new` prints the whole recipe first, as the dialog shows it, and one of your own that has steps goes ahead only with `--yes`. `recipe` runs a machine's steps again, in the running guest. `show` says how its steps went.

## What needs a real guest to prove

- Leaving folders out of scanning: that `Add-MpPreference` as SYSTEM takes a folder with Tamper Protection on, that `Get-MpComputerStatus` still says real-time protection is on, and that Windows Security lists the folder under exclusions.
- Lean Windows: that the Appx cmdlets work in the specialize pass as SYSTEM, that what it removed doesn't come back at the first logon or with a Store update, that each policy takes effect on Home, Pro and IoT Enterprise LTSC, and how much it saves.
- winget on a freshly installed Windows 11: that `Add-AppxPackage -RegisterByFamilyName` makes it appear, how long that takes, and that the ids in the recipes are right. This is the first thing to try.
- That the task as the guest's account runs with administrator rights, and that winget works from it.
- What winget says and returns for a package that's already installed, which the steps take as done.
- The Ubuntu guest being ready: that its marker is older than the boot after the first boot's restart, that the GPU step of that boot has ended before the first step, and that SSH answers by then.
- Each package in the Ubuntu recipes, and in particular that the CUDA toolkit from Ubuntu's archive doesn't bring a driver of its own over the host-matched one.
