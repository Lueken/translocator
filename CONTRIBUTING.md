# Contributing to Translocator

The launcher is free software under **GPL-3.0** (see LICENSE). Fork it, change
it, redistribute it, sell it. The only obligation is the GPL's own: a modified
version you distribute stays GPL-3.0 and ships its source.

### What the licence covers, precisely

It covers **this code**: the source in this repository and binaries built from
it. That is the whole of its reach.

It does not restrict anyone from writing a Vintage Story launcher. It could
not; copyright protects expression, not ideas, and there is nothing ownable
about the concept of a launcher. A dozen already exist on ModDB and more are
welcome. If you read this repository, understand how something works, and
write your own implementation, that is yours and this licence has no claim on
it. What GPL-3.0 governs is copying from this codebase: build on this source
and your work carries the same terms.

The **Modpack Hub** is a separate service, not covered here. Its source is not
published. The launcher reaches it over a public read API, and nothing stops
anyone running a hub of their own.

### What this replaced, and why

The project previously carried GPL-3.0 with the Commons Clause stapled on top.
That was neither open source nor internally consistent: GPLv3 section 7 lets a
recipient remove any term that is a further restriction, and a no-sell clause
bolted onto a licence that expressly permits selling is exactly such a term. It
produced an argument rather than a protection.

The clause is gone rather than replaced with a better-drafted one. A noncompete
that actually worked would have forbidden the community fork along with the
competitor, and for a project one person maintains, nobody being able to carry
it on is the worse risk. The thing that is actually a business, the Hub, is
protected by being a separate closed service rather than by a clause in a
licence.

## Contributions: sign your work

This project uses the **Developer Certificate of Origin**. There is no
contributor licence agreement, no copyright assignment, and no relicensing
right handed to anyone. You keep your copyright. Your contribution is licensed
to everyone under GPL-3.0, the same terms as the rest of the code, and it stays
that way. Under a copyleft licence that is not merely a promise: no future
maintainer can close your work either.

Sign off each commit:

```
git commit -s
```

which appends a line to the message:

```
Signed-off-by: Your Name <your@email.example>
```

Use your real name and an email you can be reached at. CI checks that every
commit in a pull request carries one.

### What you are certifying

The full text, reproduced verbatim:

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.

Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

### Why a DCO and not a CLA

An earlier version of this file asked contributors for a perpetual,
irrevocable right to relicense their work "under any license terms the
Licensor selects", with attribution at the project's discretion. That was too
much to ask of somebody fixing a bug, and it was written out of a solo
developer's reflex rather than a considered need.

The real problem a CLA solves is being able to change licence later without
tracking down every past contributor. That is worth taking seriously, and it is
also the thing being given up here deliberately. Under GPL-3.0 with a DCO, this
project cannot be relicensed without the agreement of everyone whose code is
still in the tree, including you. That is the point rather than an oversight:
the guarantee is only worth something if it binds the maintainer too.

Two things the DCO does not do, so nobody is surprised later. It carries no
patent grant from contributors, unlike an Apache-style CLA. And it is an
assertion by you, not a transfer to anyone; nobody gains rights in your work
beyond GPL-3.0.

### Attribution

Accepted contributions are credited to you in the commit history and, for
anything substantial, in the release notes. If you would rather not be
credited, say so and you will not be.

## Practical notes

- Discuss substantial changes in an issue or on Discord before writing code;
  the project has strong design conventions (see `docs/`).
- Official builds are produced and signed only by the Licensor. See
  `docs/releasing.md` for how a release is cut and verified.
- Security findings are welcome and taken seriously. If something looks
  sensitive, raise it privately first.
