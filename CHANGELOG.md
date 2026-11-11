# Changelog

## [0.7.1](https://github.com/borovikovd/supermonkey/compare/v0.7.0...v0.7.1) (2026-11-11)


### Bug Fixes

* **accounts:** an account removed while its sign-in waits no longer comes back signed out ([06f0133](https://github.com/borovikovd/supermonkey/commit/06f01338caca34ef0b878cca03c4262097759478))

## [0.7.0](https://github.com/borovikovd/ubermensch/compare/v0.6.1...v0.7.0) (2026-11-08)


### ⚠ BREAKING CHANGES

* the new bundle id gives the app a new data folder, so it starts fresh: accounts are added again and mail syncs back. The old folder is left as it was. Drafts written before the rename carry the old header and are treated as drafts from elsewhere.

### Features

* rename the app to Supermonkey ([26c3e09](https://github.com/borovikovd/ubermensch/commit/26c3e098c96aab6f2363770b2ef56884666cd23d))


### Bug Fixes

* **accounts:** a sign-in replaced by another no longer shows "Sign-in cancelled" ([685cd28](https://github.com/borovikovd/ubermensch/commit/685cd28762c7808290a86a6370bd6060d07829f1))
* close nine races a concurrency review found ([507f5a8](https://github.com/borovikovd/ubermensch/commit/507f5a8308c9234e8d3452fb73bd089024c89ecb))
* close seven more races from a third concurrency review ([6dcdc7a](https://github.com/borovikovd/ubermensch/commit/6dcdc7abcc5bb0d4de87b00388867667a6cec1e4))
* close six more races from a fourth concurrency review ([3505656](https://github.com/borovikovd/ubermensch/commit/35056566aaef7d42e4b4ddfab692a736792d0c82))
* close twelve more races from a second concurrency review ([05a7a0a](https://github.com/borovikovd/ubermensch/commit/05a7a0a1081ba31afe2f1d9a030213be1c645d4c))
* **drafts:** a new message deleted in Gmail while open reaches Gmail again ([ae80b35](https://github.com/borovikovd/ubermensch/commit/ae80b3526dc4a13e4a99e58612e88fd9a9c4cbea))
* **drafts:** a reply discarded while its quote downloads is never written to Gmail ([2ebc676](https://github.com/borovikovd/ubermensch/commit/2ebc676b05c2fdb53ea345e38e2e29ba68d540ec))
* **drafts:** removing an account while a send waits for a file no longer switches back to it ([5d160da](https://github.com/borovikovd/ubermensch/commit/5d160da8b1543234733016e658b4794a8371805e))
* **reading:** a message opening as it downloads grows into its body without shrinking first ([70fde5e](https://github.com/borovikovd/ubermensch/commit/70fde5e1af9d50ec9f4697eb29ea2f9da0ac22fb))
* **search:** Enter on the sign-in screen signs in with a search left open ([56a3f8e](https://github.com/borovikovd/ubermensch/commit/56a3f8e741e46d939cef769a530a104349cf1997))
* **send:** Undo answers at once when the account signs out as the send goes out ([a66ba5c](https://github.com/borovikovd/ubermensch/commit/a66ba5cddd58f250111a4c54aa027964ae0ba372))

## [0.6.1](https://github.com/borovikovd/ubermensch/compare/v0.6.0...v0.6.1) (2026-11-07)


### Bug Fixes

* **accounts:** a reply open when its account signs out stays on screen until closed ([2b8e1da](https://github.com/borovikovd/ubermensch/commit/2b8e1da9650f24b873155c857fbdce61308d47f3))
* **accounts:** removing the account on screen closes its open composer ([7b650b7](https://github.com/borovikovd/ubermensch/commit/7b650b71091ad77b654b6f2ce28f24c101866e49))
* **accounts:** removing the account on screen shows the next account on the rail ([dbf30ee](https://github.com/borovikovd/ubermensch/commit/dbf30ee4a954bb23bb6dbd0f63d911d483d95f11))
* **actions:** an action the app can't read no longer holds up every action after it ([78e34f0](https://github.com/borovikovd/ubermensch/commit/78e34f0d22a60192980b489438f2564221de529e))
* **app:** quit within 10 s of Cmd+Q or a logout even if the window stops answering ([ab10a9e](https://github.com/borovikovd/ubermensch/commit/ab10a9e13f443e638170a4a521076afa9b444b1b))
* **app:** show the window only once the app is ready, and always within 5 s ([a34617b](https://github.com/borovikovd/ubermensch/commit/a34617b5c9b8b0c6246de277697c7b410b0a780c))
* **compose:** a draft deleted elsewhere as you open it closes its composer and says so ([72240a0](https://github.com/borovikovd/ubermensch/commit/72240a06325eb41a28d2183dfa4697b0b95e6c54))
* **compose:** a recipient removed with its × stays removed ([8e3c311](https://github.com/borovikovd/ubermensch/commit/8e3c311701b5ddd1c3586b18f41a3a145fff36a3))
* **compose:** a send waiting for a file still goes when its composer is closed ([ae2af1d](https://github.com/borovikovd/ubermensch/commit/ae2af1d44982a44a41f423212933a3bf4a84d26e))
* **compose:** an Esc pressed as a composer opens is no longer undone by its focus ([73f0f6e](https://github.com/borovikovd/ubermensch/commit/73f0f6e02770c246660b88c3890c0823c854bd66))
* **compose:** an Esc pressed right after Enter or a field shortcut is no longer undone ([6e6977c](https://github.com/borovikovd/ubermensch/commit/6e6977c57b6ee3ef08ad59156d237607664f01a3))
* **compose:** Cmd+Q waits for a composer's close that waits on a file ([0c5e05b](https://github.com/borovikovd/ubermensch/commit/0c5e05b76fac0b606bbc12a034bb0a12fcabad6c))
* **compose:** discarding a draft while its send waits for a file cancels the send ([cbd23a5](https://github.com/borovikovd/ubermensch/commit/cbd23a5efa0d00a0e12de5a4045c435c71b7fccc))
* **compose:** keep an address typed in To, Cc, or Bcc when closing or sending while a file attaches ([60206fc](https://github.com/borovikovd/ubermensch/commit/60206fc4ad96be4caeefa813a0718887998db164))
* **compose:** links in a reply's quoted text open in your browser ([3aef3ae](https://github.com/borovikovd/ubermensch/commit/3aef3ae86b1dbb3668bab6b8f4d69bcbafccb9b2))
* **compose:** refuse a file over 25 MB before reading it ([2384f59](https://github.com/borovikovd/ubermensch/commit/2384f595581bb00b45e191ec5b12ae9ddad05e24))
* **compose:** reopening a draft whose close waits for a file keeps its text and file ([e659da0](https://github.com/borovikovd/ubermensch/commit/e659da06e6a9fb5674b77a6c39988e41e4085800))
* **compose:** send every file attached while a send waits for another ([3551f8a](https://github.com/borovikovd/ubermensch/commit/3551f8af1fe22117aff3a5357e6d4eac2ed95716))
* **compose:** the same file can be attached twice ([b348851](https://github.com/borovikovd/ubermensch/commit/b348851cc8e3a8c6a39674601cc6b6a3169ebb56))
* **drafts:** an edit still waiting to reach Gmail survives the draft's deletion elsewhere ([6eb42e8](https://github.com/borovikovd/ubermensch/commit/6eb42e8fb9dcdfe7e24e3d122a3f61687a82bd33))
* **drafts:** j and k in a conversation opened from Drafts open the next draft's thread ([9c5929c](https://github.com/borovikovd/ubermensch/commit/9c5929cde1ce04ced05fa1636c8d4d0c37a01897))
* **keyboard:** g # and g ! go to Trash and Spam instead of trashing or spamming ([58adb09](https://github.com/borovikovd/ubermensch/commit/58adb09b1022b813be8f12d970b877c369509bf2))
* **list:** j on the last conversation and k on the first keep your selection ([79d6512](https://github.com/borovikovd/ubermensch/commit/79d65125e2f7342171589408c654bec291aa4626))
* **list:** the list no longer jumps back to the highlighted row while you scroll through it ([6fcee86](https://github.com/borovikovd/ubermensch/commit/6fcee8652b86eea30dd79087c3528c6495a9f399))
* **offline:** find out within 10 s that Gmail is back, even when nothing else calls it ([d8795f3](https://github.com/borovikovd/ubermensch/commit/d8795f31e2221f1a07d01f96eb37d2cc044f1d82))
* **quit:** Cmd+Q saves what you just typed and did before the app quits ([dfbc789](https://github.com/borovikovd/ubermensch/commit/dfbc7898f394750950e4b3a40d8d2bff1071028b))
* **reading:** a reply open when its conversation is deleted from Gmail closes and keeps its draft ([0ec43a5](https://github.com/borovikovd/ubermensch/commit/0ec43a55840da4174375d9c89c4dbc61057889de))
* **reading:** a sent reply stays in the conversation, marked Sent, until its message arrives ([7a72988](https://github.com/borovikovd/ubermensch/commit/7a72988e966f6f3b244652636a42893938362432))
* **reading:** list the attachments of a message forwarded as an attachment ([a3f6786](https://github.com/borovikovd/ubermensch/commit/a3f67860e6eda2de9a9f4d753ada4b8a029f0c1d))
* **reading:** open and forward small files Gmail serves inside the message ([b4a5e4a](https://github.com/borovikovd/ubermensch/commit/b4a5e4ae8b89d830be99c386d4707ba554060114))
* **reading:** r on the list shows the reply below a long message ([784ef29](https://github.com/borovikovd/ubermensch/commit/784ef29c4d9fd3135712ce67b5af4da5a8a55f0f))
* **reading:** r, Enter, and f no longer lock the keyboard in a conversation that is gone ([2abe0e7](https://github.com/borovikovd/ubermensch/commit/2abe0e7a5aaa3fcf2b1d53ea698877f61e1a0033))
* **reading:** say why an attachment could not be opened ([f5ba1b0](https://github.com/borovikovd/ubermensch/commit/f5ba1b014cae27fc77f10e404de3dc001bcfa652))
* **reading:** show files Apple Mail sends inline, such as a PDF, as attachments ([3a8a156](https://github.com/borovikovd/ubermensch/commit/3a8a1567368000cd8e68dd6695450a41fc9da137))
* **reading:** show mail declared as ISO-2022-KR or HZ-GB-2312 instead of one blank glyph ([89d31de](https://github.com/borovikovd/ubermensch/commit/89d31de162b606b6cbf6359a3a19df08573b93d6))
* **reading:** show photos the message body doesn't show as attachments ([b6e6f6b](https://github.com/borovikovd/ubermensch/commit/b6e6f6ba3dd3500aee0a431437f8b6afda1bf426))
* **search:** find the words of an HTML message whose tag has a stray quote ([86aeb64](https://github.com/borovikovd/ubermensch/commit/86aeb64f20573c4181ae03e8a2ffa06ccc908206))
* **search:** list every Gmail hit of a broad search, so its next page loads ([4e7e15d](https://github.com/borovikovd/ubermensch/commit/4e7e15d5c796a41ef88bc1fa04d61eab48feb6e3))
* **search:** show every message of a conversation opened from an in:trash or in:spam search ([975f72a](https://github.com/borovikovd/ubermensch/commit/975f72abad1c2b655eb45c43fa0694690ffe569c))
* **search:** stop a search for is:list alone from listing the whole mailbox ([ea0d472](https://github.com/borovikovd/ubermensch/commit/ea0d4725e1ed0866de234870d600b5a9e10f1a24))
* **send:** a send settled as sent with no message no longer stays as "Sent" in the thread ([9a5ba52](https://github.com/borovikovd/ubermensch/commit/9a5ba52fef7187f2c3523a8e2198f7865ce9eda7))
* **send:** an undo held on a send whose sign-in token couldn't be refreshed resolves at once ([48a91f4](https://github.com/borovikovd/ubermensch/commit/48a91f424db363e6a4b472f9cf5f84c371508f98))
* **store:** keep the mail database when it is only locked or can't be opened ([fea05f5](https://github.com/borovikovd/ubermensch/commit/fea05f5c3c910e702eab93f71b65a44e451947e2))
* **sync:** the "Can't reach Gmail" dot clears once Gmail answers after a relaunch ([eaba611](https://github.com/borovikovd/ubermensch/commit/eaba611fa0418a0cc3556d51e5dfbd6957a992c5))
* **triage:** s, u, and l on Select all count every selected conversation ([76c93c1](https://github.com/borovikovd/ubermensch/commit/76c93c141e57e100b088c23ff367af53841ea0be))
* **triage:** Shift+E on trashed or spam mail found by search takes it out of Trash or Spam ([d55c213](https://github.com/borovikovd/ubermensch/commit/d55c2134ede3e4ab355b149510ad48b61935518a))
* **unsubscribe:** block and unsubscribe act on their own account after a switch ([4a7b4d0](https://github.com/borovikovd/ubermensch/commit/4a7b4d01f22a8290559156936112f693c3fed026))

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
