# Changelog

## [0.6.0](https://github.com/borovikovd/ubermensch/compare/v0.5.2...v0.6.0) (2026-11-07)


### Features

* **palette:** list only the commands that work where it opens ([de1d6d7](https://github.com/borovikovd/ubermensch/commit/de1d6d730f385ac872feb5f903546da7acf63be6))


### Bug Fixes

* **compose:** close a reply when search closes or a new query replaces it ([3b276b0](https://github.com/borovikovd/ubermensch/commit/3b276b016f7ff5ea2f0591b8c44e67f2a66dfca3))
* **compose:** mark every address that can't be sent to, and refuse Send before the composer closes ([8cd55b9](https://github.com/borovikovd/ubermensch/commit/8cd55b974684c350953eb8a2c9f6c1128ddf11d7))
* **compose:** quote the whole message in a reply or forward started while it downloads ([0ce91a2](https://github.com/borovikovd/ubermensch/commit/0ce91a271766861a373e78145edf9f13c9dce4d1))
* **drafts:** keep a draft edit that is still waiting to reach Gmail ([c66fe68](https://github.com/borovikovd/ubermensch/commit/c66fe680a0bae622196eaae7b5c160ce1a855af9))
* **keyboard:** Esc closes a sheet from its fields, or over a composer ([25ccc37](https://github.com/borovikovd/ubermensch/commit/25ccc379737d86fe4c263ab50a53fb7970013dc8))
* **privacy:** with remote images off, block web fonts and the reply quote's images too ([bb17cb2](https://github.com/borovikovd/ubermensch/commit/bb17cb2d9d235467253f34ec5af09e6849f99dbb))
* **reading:** show Russian and other non-Latin mail that Gmail labels with an old charset ([5698ec3](https://github.com/borovikovd/ubermensch/commit/5698ec3785975721298823fd286a77bf655c6eeb))
* say why when the app can't do what you asked, and bring back a draft it couldn't send ([605f851](https://github.com/borovikovd/ubermensch/commit/605f8515fade0563020a4b4bce032c44f89dd0c4))
* **search:** load more Gmail results where they belong, not at the list's end ([4cc60f3](https://github.com/borovikovd/ubermensch/commit/4cc60f31d37a5024f643d8a68f11e6b6b0432c45))
* **search:** run a search's or view's Gmail listing once the network returns ([24f6003](https://github.com/borovikovd/ubermensch/commit/24f6003253918733262077afb305f920b4f2a2b5))
* **splits:** keep the splits sheet open when a rule can't be used ([1bb58e4](https://github.com/borovikovd/ubermensch/commit/1bb58e45c8383bf92c01ddcfdcec60dab950dc6b))
* **triage:** never act on a conversation that is no longer on screen ([241a15e](https://github.com/borovikovd/ubermensch/commit/241a15e71228b1da5b795aa87619c987e8aa5021))

## [0.5.2](https://github.com/borovikovd/ubermensch/compare/v0.5.1...v0.5.2) (2026-11-05)


### Bug Fixes

* **reading:** show a message's preview while its body downloads ([e1e684d](https://github.com/borovikovd/ubermensch/commit/e1e684dcfaedf86f49c68ed00ecbbf5d4abeb7e3))
* **search:** show only matching mail for in:sent, is:unread, and other Gmail operators ([6ee548b](https://github.com/borovikovd/ubermensch/commit/6ee548b397b61fcc0a6fd1b3b61f2b2df278603c))

## [0.5.1](https://github.com/borovikovd/ubermensch/compare/v0.5.0...v0.5.1) (2026-11-05)


### Chores

* release 0.5.1 ([26be74b](https://github.com/borovikovd/ubermensch/commit/26be74b086a583e72b834b5e3ddc498e23301069))

## [0.5.0](https://github.com/borovikovd/ubermensch/compare/v0.4.0...v0.5.0) (2026-11-05)


### ⚠ BREAKING CHANGES

* local data starts over. After updating, sign in to your accounts again; mail syncs back from Gmail.

### Refactoring

* rewrite the Rust core ([cbff0f6](https://github.com/borovikovd/ubermensch/commit/cbff0f6449386ee350b7a05b6e6fe374912b6961))

## [0.4.0](https://github.com/borovikovd/ubermensch/compare/v0.3.0...v0.4.0) (2026-11-04)


### Features

* add Select all (Cmd+Shift+A) and Select all from here (Cmd+A) ([0397351](https://github.com/borovikovd/ubermensch/commit/039735181151db454e5bad87347ed03fc6bd710f))
* block senders and domains with Gmail filters ([578a73a](https://github.com/borovikovd/ubermensch/commit/578a73a5324e46efb4293d7bc14205626678e7d8))
* make search look like Superhuman's ([518c17d](https://github.com/borovikovd/ubermensch/commit/518c17df5664c9eeca59a428d32ba9d4f5e75260))
* unsubscribe with Cmd+U, as in Superhuman ([f6691cb](https://github.com/borovikovd/ubermensch/commit/f6691cb17e6781960d7d0edbe7ed264bddd8c427))

## [0.3.0](https://github.com/borovikovd/ubermensch/compare/v0.2.1...v0.3.0) (2026-11-04)


### Features

* make the list highlight follow the mouse, as in Superhuman ([56ed2b0](https://github.com/borovikovd/ubermensch/commit/56ed2b02a07b25a68ccfd609338c106599efb85a))

## [0.2.1](https://github.com/borovikovd/ubermensch/compare/v0.2.0...v0.2.1) (2026-11-04)


### Bug Fixes

* open Settings when clicking the account already on screen ([bae859c](https://github.com/borovikovd/ubermensch/commit/bae859c65d36f1bb9b9cdf2d203b9491543d445f))

## [0.2.0](https://github.com/borovikovd/ubermensch/compare/v0.1.2...v0.2.0) (2026-11-04)


### Features

* mark read on Mark Done and add All Mail (g a), as in Superhuman ([01eec7f](https://github.com/borovikovd/ubermensch/commit/01eec7f5ae7d47dfa933e33473a9b5e7b565bce6))
* show a sign-in screen for a signed-out account ([94f7fd5](https://github.com/borovikovd/ubermensch/commit/94f7fd5ebc9945db7efcd65bc302cb3e7b4c5b86))

## [0.1.2](https://github.com/borovikovd/ubermensch/compare/v0.1.1...v0.1.2) (2026-11-04)


### Bug Fixes

* open links in emails in the browser ([341424c](https://github.com/borovikovd/ubermensch/commit/341424c4a805c8b66a2854eebeb907a808d38234))

## [0.1.1](https://github.com/borovikovd/ubermensch/compare/v0.1.0...v0.1.1) (2026-11-04)


### Bug Fixes

* keep email text readable when macOS is in dark mode ([4cc5fc4](https://github.com/borovikovd/ubermensch/commit/4cc5fc4c39a3805e7f9f461df035c06d4f35dd51))
* stop asking for the Mac login password at launch ([94f89c8](https://github.com/borovikovd/ubermensch/commit/94f89c81226b2c5fba8cc41799b13cabd8b50d05))

## 0.1.0 (2026-11-04)


### Features

* first release of Ubermensch, a keyboard-first Gmail client for macOS ([c7f09e3](https://github.com/borovikovd/ubermensch/commit/c7f09e33cbbc06212d73c308cb25bfa8a62a5084))
