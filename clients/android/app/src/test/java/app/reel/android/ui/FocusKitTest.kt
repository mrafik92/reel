package app.reel.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsNotFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.pressKey
import androidx.compose.ui.unit.dp
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import app.reel.android.DeviceKind
import app.reel.android.LocalDeviceKind
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The D-pad focus kit on a TV: initial focus, arrow keys, and focus restored after Back. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w960dp-h540dp-television")
@OptIn(ExperimentalTestApi::class)
class FocusKitTest {
    @get:Rule
    val rule = createComposeRule()

    private lateinit var nav: NavHostController

    private fun setContent(kind: DeviceKind = DeviceKind.Tv) = rule.setContent {
        nav = rememberNavController()
        CompositionLocalProvider(LocalDeviceKind provides kind) {
            NavHost(nav, startDestination = "row") {
                composable("row") {
                    val memory = rememberFocusMemory()
                    RestoreFocus(memory)
                    LazyRow(state = rememberLazyListState(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        items((0 until 6).toList(), key = { it }) { i ->
                            FocusCard(
                                onClick = { nav.navigate("detail/$i") },
                                modifier = Modifier.size(100.dp, 150.dp).focusMemory(memory, "card$i").testTag("card$i"),
                            ) { Text("Card $i") }
                        }
                    }
                }
                composable("detail/{i}") { Text("Detail") }
            }
        }
    }

    @Test
    fun firstCardIsFocusedOnEntry() {
        setContent()
        rule.onNodeWithTag("card0").assertIsFocused()
    }

    @Test
    fun arrowsMoveAndOkOpens() {
        setContent()
        rule.onNodeWithTag("card0").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("card1").assertIsFocused()
        rule.onNodeWithTag("card1").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("card2").assertIsFocused()
        rule.onNodeWithTag("card2").performKeyInput { pressKey(Key.DirectionCenter) }
        rule.waitForIdle()
        assertEquals("detail/{i}", nav.currentDestination?.route)
    }

    @Test
    fun backRestoresTheCardYouLeftFrom() {
        setContent()
        rule.onNodeWithTag("card0").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("card1").performKeyInput { pressKey(Key.DirectionRight) }
        rule.onNodeWithTag("card2").assertIsFocused()
        rule.runOnIdle { nav.navigate("detail/2") }
        rule.waitForIdle()
        rule.runOnIdle { nav.popBackStack() }
        rule.waitForIdle()
        rule.onNodeWithTag("card2").assertIsFocused()
        rule.onNodeWithTag("card0").assertIsNotFocused()
    }

    @Test
    fun phonesDontFocusAnything() {
        setContent(DeviceKind.Phone)
        rule.onNodeWithTag("card0").assertIsNotFocused()
    }
}
