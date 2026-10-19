# Preparing a release

Build the download **before publishing**. The Windows tester bundle workflow
creates an unpublished draft with its ZIP, checksum and build information after
all checks pass. Review that draft and publish it when ready. Publishing does not
start another build. This applies to tags containing the draft-release workflow;
the initial v0.3.0 release used the earlier build-on-publication workflow.

## Prepare the version

1. Update `crates/bundle/Cargo.toml` and its `Cargo.lock` package entry to the
   intended version. Internal library versions can remain unchanged.
2. Merge the reviewed change and check main's CI. Choose that exact commit for
   the release; the bundle version must match the tag without its leading `v`.
3. Create and push the version tag. For example, after replacing the version and
   commit below with the reviewed values:

   ```sh
   git tag -a v0.3.1 REVIEWED_COMMIT_SHA -m "Afterhours 0.3.1"
   git push origin v0.3.1
   ```

The tag push starts **Windows tester bundle**. It checks the tag/version, runs
formatting, Clippy and workspace tests, builds the standalone Windows executable,
then smoke-tests and packages it. After checking the remote tag still identifies
the built commit, it creates a **draft** containing:

- `Afterhours-windows-x64.zip`
- `SHA256SUMS.txt` (checksums of both ZIP files)
- `build-info.json`
- `Afterhours-contract.zip`: the executable's `version --json` as
  `version.json` and the JSON Schemas of its command-line records under
  `schema/`, for the desktop launcher's generated types. Packaging refuses a
  schema that differs from what the built executable prints. The tester ZIP
  carries the same `schema/` folder.

An initial build can take a while, especially without a reusable dependency
cache. The release stays unpublished throughout preparation. PR builds and
manual runs with an empty `release_tag` produce development Actions artifacts
only. A manual development run on main can also populate main's build cache.

## Review and publish

Wait for the workflow to succeed, then open the draft in GitHub Releases. Review
the generated notes, describe supported changes and remaining limitations, and
verify the four assets are present. The build manifest must identify the tag's
commit and a clean source tree. Download the ZIP and verify its checksum before
sharing it.

Publish the prepared draft as a normal release (or mark it as a pre-release when
that is intended). The attached assets are already built; publication makes them
available immediately. A draft is the preparation stage, separate from GitHub's
public pre-release designation.

Do not start with GitHub's **Publish release** button for a new version: that
would expose a release page before its bundle was prepared. Do not move a
published tag or replace a published binary.

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
`pwsh -File tools/Test-TerminalDraft.ps1`.
