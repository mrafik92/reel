package app.reel.android.ui.search

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.pressKey
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ItemSummary
import app.reel.android.data.PersonHit
import app.reel.android.data.SearchAll
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** Search on a TV: the box has focus on entry, Down reaches the results, OK opens one. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w960dp-h540dp-television")
@OptIn(ExperimentalTestApi::class)
class SearchFocusTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun boxFocusedThenDownToResults() {
        var opened: Long? = null
        var person: String? = null
        val results = SearchAll(
            library = listOf(ItemSummary(7, "movie", "Dune"), ItemSummary(8, "movie", "Dune 2")),
            people = listOf(PersonHit(5, "Ava Stone")),
        )
        rule.setContent {
            CompositionLocalProvider(LocalDeviceKind provides DeviceKind.Tv) {
                SearchContent(
                    query = "dune",
                    state = SearchState.Results("dune", results),
                    onQuery = {},
                    onSubmit = {},
                    onRetry = {},
                    onOpen = { opened = it.id },
                    onPerson = { person = it.name },
                )
            }
        }
        rule.waitForIdle()
        rule.onNodeWithTag("search-field").assertIsFocusedSomewhere()
        rule.onNode(isFocused()).performKeyInput { pressKey(Key.DirectionDown) }
        rule.waitForIdle()
        rule.onNode(isFocused() and hasAnyAncestor(hasTestTag("result-7"))).assertExists()
        rule.onNode(isFocused()).performKeyInput { pressKey(Key.Enter) }
        rule.waitForIdle()
        assertEquals(7L, opened)
        rule.onNode(isFocused()).performKeyInput {
            pressKey(Key.DirectionDown)
        }
        rule.waitForIdle()
        rule.onNode(isFocused()).performKeyInput { pressKey(Key.Enter) }
        rule.waitForIdle()
        assertEquals("Ava Stone", person)
    }

    /** The text field's focusable node sits inside the tagged one. */
    private fun androidx.compose.ui.test.SemanticsNodeInteraction.assertIsFocusedSomewhere() {
        rule.onNode(isFocused() and (hasTestTag("search-field") or hasAnyAncestor(hasTestTag("search-field")))).assertExists()
    }
}
