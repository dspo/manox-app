# Built-in extension skills

Skills that ship inside the app live here, one directory per built-in
extension: `skills/<extension>/SKILL.md`, where `<extension>` is the
`name` in `pi-harness/src/extensions/registry.ts`.

- The extension declares the directory in its `skillDirs`; only enabled
  extensions hand their roots to pi's `additionalSkillPaths`. Disabling an
  extension in `~/.steer/settings.json` stops its skill loading too.
- The factory resolves the roots through `skillsBase()`: source runs read
  this tree, the packaged app reads `Contents/Helpers/skills`
  (`/usr/lib/steer/skills` on Linux) beside `pi-acp`. `bun run build`
  (`scripts/build.ts`) copies the tree into `dist/` and fails the build if any
  `SKILL.md` here did not land next to the executable.
- This file is not a skill (it is not named `SKILL.md`), so pi ignores it.

The first real skill (`skills/todos/SKILL.md`) belongs to the A2 issue;
B3 ships only the mechanism and this layout.
