# Preparing a release

A release is prepared by three workflows, and a person does two things: review
the notes, and publish the draft.

1. **Prepare release** (run by hand) opens a pull request that bumps the
   bundle version, with tester-facing notes drafted in its description.
2. Merging that pull request is the go-ahead. Once CI passes on the merge
   commit, **Tag release** tags it `v<version>` and starts **Windows tester
   bundle**.
3. **Windows tester bundle** builds, checks and packages the bundle and creates
   an unpublished **draft** release carrying the notes from the pull request.
4. The owner reviews the draft and publishes it. Nothing publishes on its own.

The draft is built **before publishing**: publishing starts no build. This
applies to tags containing the draft-release workflow; the initial v0.3.0
release used the earlier build-on-publication workflow.

## One-time setup

Prepare release opens its pull request with the workflow's own token. Allow
that in Settings → Actions → General → Workflow permissions: tick **Allow
GitHub Actions to create and approve pull requests**. The default permission
can stay read-only, because each workflow asks for what it needs. No secret
or personal token is involved.

## 1. Prepare the release

Actions → **Prepare release** → Run workflow on main. Enter the version, such
as `0.5.0`, or leave it blank for the next patch version. Tick **dry run** to
see the notes and the version change in the run's summary without opening
anything. The workflow refuses a version that is not newer than the current
one, a tag that already exists, and a `feature/release-<version>` branch that
already exists.

It sets the version in `crates/bundle/Cargo.toml` and its `Cargo.lock` package
entry (internal library versions stay as they are), pushes
`feature/release-<version>`, opens **Release \<version\>** and starts CI on it.
Pull requests opened with the workflow token start no CI by themselves, which
is why it is dispatched; those checks count for the merge. If they are missing,
run `gh workflow run ci.yml --ref feature/release-<version>`.

### How the notes are drafted

The description holds the notes between `<!-- release-notes:start -->` and
`<!-- release-notes:end -->`, then a review section for the owner that never
reaches the release. The notes follow the layout of the v0.4.0 notes:

- a summary line to replace, and the download instructions;
- **Added**, **Changed** and **Fixed**, one entry per pull request merged into
  main since the previous version tag. A `kind:feature` label puts it under
  Added, `kind:bug` under Fixed and `kind:change` under Changed. Without one of
  those, the branch decides: `feature/` is Added, `bugfix/` is Fixed, anything
  else is Changed. Release pull requests (`feature/release-*`) are left out;
- each entry is the pull request's title in bold and a link to it. A pull
  request whose description has a `## Release note` section (or `## Release
  notes`, `## For testers`) gets that text after its title, so the best place
  for tester-facing wording is the pull request itself;
- **Known limits**, copied from the previous release's notes;
- the closing paragraphs and links of every tester release.

Edit the notes in the pull request's description: the draft takes them from
there when it is built, so edits up to the merge (and until the build reaches
its last step) are kept. If more pull requests merge before the release pull
request does, close it, delete its branch and run Prepare release again, so
the notes include them.

CI's **Release scripts** job tests these scripts offline
(`pwsh -File tools/Test-Release.ps1`) and, on every pull request, shows the
notes the next release would start with in its run summary.

## 2. Merge

Check the notes, CI and the version, then merge. The required checks are strict:
if main moved, use **Update branch** first.

**Tag release** runs when CI finishes on main. It acts only on the commit that
changed the bundle version, only after CI passed on that commit, and never
moves or reuses a tag: it refuses a version that is not newer than the latest
published release. It pushes an annotated `v<version>` tag and dispatches
**Windows tester bundle** for it, because a tag pushed with the workflow token
starts no workflow by itself. CI runs on main are never cancelled by later
pushes, so every merge commit gets its verdict.

If CI failed on the merge commit, fix the cause; re-running the failed jobs
starts Tag release again once they pass. To tag by hand after CI passed, use
Actions → Tag release → Run workflow with the merge commit's full SHA.

## 3. The draft

**Windows tester bundle** checks the tag/version, runs formatting, Clippy and
workspace tests, builds the standalone Windows executable, then smoke-tests and
packages it. After checking the remote tag still identifies the built commit,
it creates a **draft** containing:

- `Afterhours-windows-x64.zip`
- `SHA256SUMS.txt` (checksums of both ZIP files)
- `build-info.json`
- `Afterhours-contract.zip`: the executable's `version --json` as
  `version.json` and the JSON Schemas of its command-line records under
  `schema/`, for the desktop launcher's generated types. Packaging refuses a
  schema that differs from what the built executable prints. The tester ZIP
  carries the same `schema/` folder.

Its text is the notes from the pull request that merged the tagged commit. A
tag without such a pull request (a hand-made release) gets the default text
and GitHub's generated list instead.

An initial build can take a while, especially without a reusable dependency
cache. The release stays unpublished throughout preparation. PR builds and
manual runs with an empty `release_tag` produce development Actions artifacts
only. A manual development run on main can also populate main's build cache.

## 4. Review and publish

Wait for the workflow to succeed, then open the draft in GitHub Releases. Read
the notes once more and verify the four assets are present. The build manifest
must identify the tag's commit and a clean source tree. Download the ZIP and
verify its checksum before sharing it.

Publish the prepared draft as a normal release (or mark it as a pre-release when
that is intended). The attached assets are already built; publication makes them
available immediately. A draft is the preparation stage, separate from GitHub's
public pre-release designation. The desktop launcher picks up a published,
non-pre-release server within the hour and opens its own pull request
(see the launcher's RELEASING.md).

Do not start with GitHub's **Publish release** button for a new version: that
would expose a release page before its bundle was prepared. Do not move a
published tag or replace a published binary.

## Releasing by hand

The workflows only automate these steps, which still work:

1. Update `crates/bundle/Cargo.toml` and its `Cargo.lock` package entry to the
   intended version in a reviewed pull request and merge it. Tag release then
   tags the merge commit once CI passes; continue at step 3.
2. Without Tag release, choose the merge commit after main's CI passed; the
   bundle version must match the tag without its leading `v`. Create and push
   the tag, which starts Windows tester bundle:

   ```sh
   git tag -a v0.3.1 REVIEWED_COMMIT_SHA -m "Afterhours 0.3.1"
   git push origin v0.3.1
   ```

## Retry a failed preparation

If no draft was created, rerun the failed workflow, or use **Actions → Windows
tester bundle → Run workflow**, select main and enter the existing version tag
in `release_tag`. The checkout uses that tag; it must include this workflow and
its helper scripts. Tag pushes and manual preparation of the same version share
a concurrency group.

An existing release is never overwritten. If draft creation or upload failed
partway through, inspect the unpublished draft and the successful packaging
step's Actions artifact. Add only missing verified files to that draft, or remove
the incomplete unpublished draft before rerunning preparation. Do not retry by
editing a published release. If source changes are needed, prepare a new reviewed
version and tag.

The draft-publication guards can be checked locally without GitHub or a game:
`pwsh -File tools/Test-TerminalDraft.ps1`. The preparation, tagging and notes
scripts: `pwsh -File tools/Test-Release.ps1`.
