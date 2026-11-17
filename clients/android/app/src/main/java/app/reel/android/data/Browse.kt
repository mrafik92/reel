package app.reel.android.data

/**
 * Browse endpoints (#188): home, libraries, items and their children, genres, played state.
 * Thin wrappers over [ReelApi.get] / [ReelApi.postEmpty] / [ReelApi.delete], so screens and
 * view-models don't repeat paths and query names.
 */

/** Sort orders of `GET /libraries/{id}/items` (the server's `sort=` values). */
enum class LibrarySort(val wire: String, val label: String) {
    Title("title", "A–Z"),
    Added("added", "Recently added"),
    Year("year", "Year"),
    Rating("rating", "Rating"),
    Runtime("runtime", "Runtime"),
}

/** What the library grid shows: one sort, at most one genre, optionally only unwatched titles. */
data class LibraryQuery(
    val sort: LibrarySort = LibrarySort.Title,
    val genre: String? = null,
    val unwatched: Boolean = false,
) {
    /** Query parameters (the server treats a missing `unwatched` as false). */
    fun params(cursor: String?, limit: Int): Map<String, Any?> = mapOf(
        "sort" to sort.wire,
        "genre" to genre,
        "unwatched" to if (unwatched) "true" else null,
        "cursor" to cursor,
        "limit" to limit,
    )

    val filtered get() = genre != null || unwatched
}

suspend fun ReelApi.home(): Home = get("/home")

suspend fun ReelApi.libraries(): List<Library> = get("/libraries")

/** One page of a library; pass the previous page's `next_cursor` to continue. */
suspend fun ReelApi.libraryItems(libraryId: Long, query: LibraryQuery, cursor: String? = null, limit: Int = 60): ItemPage =
    get("/libraries/$libraryId/items", query.params(cursor, limit))

suspend fun ReelApi.genres(libraryId: Long): List<GenreCount> = get("/genres", mapOf("library_id" to libraryId))

suspend fun ReelApi.item(id: Long): ItemDetail = get("/items/$id")

/** Seasons of a show, episodes of a season. */
suspend fun ReelApi.children(id: Long): List<ItemSummary> = get("/items/$id/children")

/** Mark watched (`POST`) or unwatched (`DELETE`); a show or season covers all its episodes. */
suspend fun ReelApi.setPlayed(id: Long, played: Boolean) {
    if (played) postEmpty("/items/$id/played") else delete("/items/$id/played")
}

/** Everything matching [q]: titles, episodes, people (`/search/all`). */
suspend fun ReelApi.searchAll(q: String, limit: Int = 30): SearchAll = get("/search/all", mapOf("q" to q, "limit" to limit))
