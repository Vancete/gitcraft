# Mergiraf in gitcraft

[Mergiraf](https://mergiraf.org) solves merge conflicts from the syntax of the code: two
branches that add different imports, functions, JSON keys or struct fields at the same place no
longer conflict. It knows Rust, Java, Kotlin, C#, Go, Python, TypeScript, JSON, YAML, TOML and
many more (`mergiraf languages` lists them).

Mergiraf is GPL-3.0; gitcraft is MIT OR Apache-2.0. gitcraft never bundles or links Mergiraf:
it runs the `mergiraf` program you install, like it runs `git`, so neither licence changes.

## The quick way: Settings → Git → Mergiraf

1. **Install.** Downloads the latest Mergiraf release for your system from
   [codeberg.org/mergiraf/mergiraf](https://codeberg.org/mergiraf/mergiraf/releases) into
   gitcraft's own folder. Shown only when no `mergiraf` is found; one on your `PATH` is always
   used first.
2. **Offer it in the conflict editor** (on by default). The conflict editor gets a
   _Solve with Mergiraf_ button: Mergiraf's answer shows in the editor, conflicts it couldn't
   solve stay to choose, and nothing is saved before you click _Save and mark resolved_.
3. **Use it in every git merge** (off by default). Git itself merges with Mergiraf, so fewer
   conflicts are left at all, in every repository and in the terminal too. Ticking it:
   - sets `merge.mergiraf.name` and `merge.mergiraf.driver` in your global git config;
   - adds a block between `# gitcraft: Mergiraf (begin)` and `# gitcraft: Mergiraf (end)` to your
     global attributes file (`core.attributesFile`, or `~/.config/git/attributes`), with a
     `*.ext merge=mergiraf` line for each file type Mergiraf knows;
   - sets `merge.conflictStyle = diff3` if you have no conflict style set: Mergiraf needs each
     conflict's base, which git's default markers leave out.

   Unticking removes the `merge.mergiraf` section and the block again; the rest of your config
   and attributes stays as it is. `merge.conflictStyle` is left as it is (diff3 markers work
   everywhere, gitcraft's conflict editor included).

## By hand

1. Install Mergiraf, so that `mergiraf --version` works in a terminal:
   - a release from <https://codeberg.org/mergiraf/mergiraf/releases>, its program put in a
     folder on your `PATH`;
   - `cargo install --locked mergiraf` (or `cargo binstall mergiraf`), with a Rust toolchain;
   - a package manager: see <https://repology.org/project/mergiraf/versions>
     (e.g. `brew install mergiraf`).

   gitcraft notices it within half a minute, without a restart.

2. Optional, to have git merge with it:

   ```sh
   git config --global merge.mergiraf.name mergiraf
   git config --global merge.mergiraf.driver 'mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L'
   git config --global merge.conflictStyle diff3
   mergiraf languages --gitattributes >> ~/.config/git/attributes
   ```

   (Use your `core.attributesFile` instead, if you set one.)

## When git merged with Mergiraf

Git prints `INFO Mergiraf: Solved 1 conflict. Review with: mergiraf review <id>` for each file
Mergiraf merged. `mergiraf review <id>` shows what it did, and `mergiraf=0 git merge …` merges
once without it. See <https://mergiraf.org/usage.html>.

## Removing it

Untick _Use it in every git merge_ first (or remove what step 2 added). A Mergiraf gitcraft
downloaded is in gitcraft's data folder, under `mergiraf/`:

- Windows: `%APPDATA%\gitcraft\data\mergiraf\`
- macOS: `~/Library/Application Support/gitcraft/mergiraf/`
- Linux: `~/.local/share/gitcraft/mergiraf/`
