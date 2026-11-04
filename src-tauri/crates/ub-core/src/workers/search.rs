//! Gmail search and view listings, filled by the search slice: runs a list
//! subscription's `threads.list`, fetches its hits' metadata, merges them
//! into the subscription (`Sub::gmail_hits`, `searching`, `next_page`), and
//! cancels a closed subscription's queued fetches (SPEC Search).
