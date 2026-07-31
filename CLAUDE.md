# mdroll

A terminal Markdown viewer. **The README is the specification.** It records
every design decision and the reasoning behind it, CI renders it as a test, and
its roadmap is where work is tracked. When behaviour and README disagree, one of
them is a bug — decide which before writing code.

## The rule about Markdown

Render **what GitHub renders, and nothing it does not**. This is not the same as
the GFM spec: GitHub renders alerts, footnotes, emoji shortcodes, math, and
front matter that no spec mentions, and it does *not* render several things
comrak offers. The reasoning, and the four comrak extensions deliberately left
off, are in the README under "What counts as Markdown here". Each of those four
has a test pinning GitHub's behaviour — if one fails, do not "fix" the test.

## Commits

- **One commit per change.** A commit that fixes two things is two commits.
- **Every commit passes `cargo test` on its own.** Verify a branch with
  `git rebase --exec 'cargo test -q' main`. Bisecting is the point.
- **A test reproducing a defect goes in the same commit as the fix**, not in a
  follow-up.
- Message style: a headline that says what the change *does*, then prose
  explaining **why it was wrong** and what the reasoning is. No bullet lists, no
  restating the diff. Look at recent history before writing one.

## Pull requests

- **Label every PR** — `rendering`, `bug`, `terminal`, `documentation`.
  `.github/release.yml` turns labels into release-note categories, and an
  unlabelled PR falls into "Other changes". Label when opening; by release time
  nobody remembers which change was which.
- **One subject per PR.** Release notes are one line per PR, so a PR carrying
  ten unrelated commits reports itself as a single line and the work disappears.
- Say *why* in the body, the same as in a commit message. The PR is what a
  reader of the release notes follows.

## Releases

A release is a bump branch and nothing else: `Cargo.toml` and `Cargo.lock` to
the new version, a `Release x.y.z` commit whose body says what changed and why
that number rather than the next one along, and a PR labelled `documentation`
listing the PRs it carries.

Then **finish it in one go** — do not stop at the open PR and hand back the
mechanical remainder:

```console
$ gh pr checks <n> --watch
$ gh pr merge <n> --merge          # not a squash: every tag points at a merge commit
$ git checkout main && git pull --ff-only
$ git tag vx.y.z <merge commit> && git push origin vx.y.z
```

Pushing the tag is the only thing that triggers
`.github/workflows/release.yml`, which builds the four target archives and
generates the notes. Until the tag is pushed there is no release, however
merged the PR is.

## Before pushing

```console
$ cargo fmt --all --check
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo test
$ for t in $(./target/debug/mdroll --list-themes); do \
      ./target/debug/mdroll --theme "$t" README.md < /dev/null > /dev/null; done
```

The last one is what CI does: the README exercises every construct, so it is the
document most likely to crash the renderer. `lefthook install` runs the first
two on commit and the third on push.

## Tests

- Test names are sentences that state the property, not `test_foo`.
- **For anything about width or line breaking, include the actual string.**
  These bugs cannot be reasoned about from a description, and every measurement
  defect found so far was invisible to tests whose text happened to avoid the
  case.
- Prefer asserting the property over a range of widths rather than one example.

## Roadmap

Defects found by reading go into the README roadmap as unchecked boxes under a
version heading, and the commit that fixes each one ticks its box. That keeps
the log and the plan in step, and keeps small defects from being forgotten
because none of them justified an issue.

## Not to do

- **Do not publish to crates.io**, and do not suggest it.
- Editing, HTML/PDF export, acting as a browser, and repository autolinks
  (`#123`) are non-goals. The README says why for each; do not reopen them
  without being asked.
- Do not add a dependency for something small and self-contained — see the
  hand-rolled `which` in `mmdc.rs` and the front matter subset in
  `frontmatter.rs`. A parser that declines what it does not understand and falls
  back to showing source is the pattern here.
