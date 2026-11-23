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
- `[steps]` is optional too. `packages` is a list of names, each its own step, and `script` runs after them. On Windows the packages are winget ids and the script is PowerShell. On Ubuntu they are apt packages and the script is run by bash.
- A name can only be letters, digits and `. _ + -`, and never starts with a hyphen. On Ubuntu, only small letters. At most 30 packages, and the script is at most 32 KiB. A file that says anything else, a word the format doesn't have, or a name that isn't a name, is refused with the line it's on.

## How the steps run

They run as a stage of their own, after the install has fully succeeded and the machine has started. A step that fails never turns an install that worked into a failed machine: the machine stays, you're told which step stopped and what the guest said, and the machine's settings have the recipe with *Show* and *Run Again*. Steps run in order, and the first that fails stops the rest. Running again starts from the top, and a package that's already there is skipped.

If the PC has no room to start the machine after its install, it stays off with its steps waiting. Start it, and run them from its settings.

Nothing from a recipe is ever put into a script as text. On **Windows**, the app sends the recipe's steps as Base64 JSON to a script of its own, which reads them as data: a package id is one argument to winget, and the script is written to a file and run from there. The agent in the guest runs as SYSTEM and takes one request at a time, and winget belongs to a user, so a short request starts the steps in a task of their own as the guest's account, with administrator rights. The app then reads a status file with quick requests, so a step that takes twenty minutes holds nothing up. On **Ubuntu**, the app runs `ssh` as root on the port forwarded to the guest, with the machine's own key, and ignoring your own SSH configuration. The command is always a fixed text. The runner (`linux/recipe/run.sh`) and the list of steps go in on stdin as files, so a name or script of a recipe is never read by a shell as anything but a name or a file to run. The guest counts as up once its first boot has finished and it has restarted, which can take a while after an install.

The Windows steps wait up to ten minutes for winget, as a new Windows 11 can take a while to register it. If it doesn't come, the step says so and the machine's settings can run the steps again later.

## On the command line

```
guesthouse-vm recipes
guesthouse-vm new NAME --iso PATH --set-up-for RECIPE [--yes]
guesthouse-vm recipe NAME [--no-wait]
```

`recipes` lists them. `RECIPE` is a recipe's key (`app/games` or `yours/mine`), its file's name or its name, and a key is needed only if two fit. `new` prints the whole recipe first, as the dialog shows it, and one of your own that has steps goes ahead only with `--yes`. `recipe` runs a machine's steps again, in the running guest. `show` says how its steps went.

## What needs a real guest to prove

- winget on a freshly installed Windows 11: that `Add-AppxPackage -RegisterByFamilyName` makes it appear, how long that takes, and that the ids in the recipes are right. This is the first thing to try.
- That the task as the guest's account runs with administrator rights, and that winget works from it.
- What winget says and returns for a package that's already installed, which the steps take as done.
- The Ubuntu guest being ready: that its marker is older than the boot after the first boot's restart, and that SSH answers by then.
- Each package in the Ubuntu recipes, and in particular that the CUDA toolkit from Ubuntu's archive doesn't bring a driver of its own over the host-matched one.
