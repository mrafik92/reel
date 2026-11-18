package app.reel.android.ui.library

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.pressKey
import androidx.navigation.compose.rememberNavController
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ItemSummary
import app.reel.android.data.Library
import app.reel.android.data.LibraryQuery
import app.reel.android.data.LibrarySort
import app.reel.android.ui.shell.LocalNavController
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The library page on a TV: D-pad reaches filters and grid, paging follows focus, dialogs work by remote. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w960dp-h540dp-television")
@OptIn(ExperimentalTestApi::class)
class LibraryFocusTest {
    @get:Rule
    val rule = createComposeRule()

    private var loadMore = 0
    private var query by mutableStateOf(LibraryQuery())
    private var opened: Long? = null

    private fun setContent(count: Int = 40) {
        val items = (1L..count).map { ItemSummary(it, "movie", "Movie $it") }
        rule.setContent {
            CompositionLocalProvider(LocalDeviceKind provides DeviceKind.Tv, LocalNavController provides rememberNavController()) {
                LibraryContent(
                    libraries = listOf(Library(1, "Movies", "movies"), Library(2, "Shows", "shows")),
                    selected = 1,
                    query = query,
                    genres = emptyList(),
                    paged = Paged(items = items),
                    onSelect = {},
                    onQuery = { query = it },
                    onLoadMore = { loadMore++ },
                    onRetry = {},
                    onOpen = { opened = it.id },
                )
            }
        }
    }

    @Test
    fun focusWalksIntoTheGridAndPagingFollowsIt() {
        setContent()
        rule.onNodeWithTag("lib-1").assertIsFocused()
        rule.onNode(isFocused()).performKeyInput { pressKey(Key.DirectionDown) }
        rule.waitForIdle()
        rule.onNode(isFocused() and hasAnyAncestor(hasTestTag("grid-1"))).assertExists()
        assertEquals(0, loadMore)
        repeat(8) {
            rule.onNode(isFocused()).performKeyInput { pressKey(Key.DirectionDown) }
            rule.waitForIdle()
        }
        assertTrue("paging should start as focus nears the end", loadMore > 0)
        rule.onNode(isFocused()).performKeyInput { pressKey(Key.Enter) }
        rule.waitForIdle()
        assertTrue(opened != null && opened!! > 1)
    }

    @Test
    fun backFromSortDialogRestoresItsTrigger() {
        setContent()
        rule.onNodeWithTag("lib-1").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("lib-2").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("sort").performKeyInput { pressKey(Key.Enter) }
        rule.onNodeWithTag("choice-A–Z").assertIsFocused()
        rule.onNodeWithTag("choice-A–Z").performKeyInput { pressKey(Key.Back) }
        rule.waitForIdle()
        rule.onNodeWithTag("sort").assertIsFocused()
    }

    @Test
    fun sortDialogByRemote() {
        setContent()
        rule.onNodeWithTag("lib-1").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("lib-2").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("sort").assertIsFocused()
        rule.onNodeWithTag("sort").performKeyInput { pressKey(Key.Enter) }
        rule.waitForIdle()
        rule.onNodeWithTag("choice-A–Z").assertIsFocused()
        rule.onNodeWithTag("choice-A–Z").performKeyInput { pressKey(Key.DirectionDown) }
        rule.onNodeWithTag("choice-Recently added").assertIsFocused()
        rule.onNodeWithTag("choice-Recently added").performKeyInput { pressKey(Key.Enter) }
        rule.waitForIdle()
        assertEquals(LibrarySort.Added, query.sort)
    }
}
