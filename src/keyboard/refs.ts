// DOM that code outside React reaches: the search field to focus, and a
// scroll request for the list. Set by the components that own them; nothing
// else lives here.

/** The search field while it is mounted. */
export const dom: { searchInput: HTMLInputElement | null } = { searchInput: null };

/** Set to show the split header at the top of the list on the next highlight move. */
export const listScroll = { headerToTop: false };
