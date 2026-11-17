package app.reel.android.ui.settings

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.LocalBringIntoViewSpec
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Logout
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import app.reel.android.BuildConfig
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.Health
import app.reel.android.data.Me
import app.reel.android.data.PlaybackPrefs
import app.reel.android.data.ReelApi
import app.reel.android.data.SessionInfo
import app.reel.android.data.preferences
import app.reel.android.data.revokeSession
import app.reel.android.data.savePreferences
import app.reel.android.data.sessions
import app.reel.android.ui.common.ChoiceDialog
import app.reel.android.ui.common.Load
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.KeepInView
import app.reel.android.ui.tv.ROW_FOCUS_SCALE
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory
import kotlinx.coroutines.launch

private class ApiBackend(private val api: ReelApi) : SettingsBackend {
    override suspend fun preferences() = api.preferences()

    override suspend fun savePreferences(p: PlaybackPrefs) = api.savePreferences(p)

    override suspend fun sessions() = api.sessions()

    override suspend fun revokeSession(id: Long) = api.revokeSession(id)

    // Signed in, so the server includes its version, git SHA and build date (`api.health()` sends no token).
    override suspend fun health(): Health = api.get("/health")
}

/** A dialog the page has open. */
private sealed interface Open {
    data object Quality : Open

    data object SubtitleMode : Open

    data class AddLanguage(val audio: Boolean) : Open

    data class EditLanguage(val audio: Boolean, val index: Int) : Open

    data class SignOutDevice(val s: SessionInfo) : Open
}

/**
 * Settings (#194): account (change server, sign out), playback preferences (saved to the account,
 * shared with the web), quality on this device, signed-in devices, About. Admin settings stay on
 * the web.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun SettingsScreen(user: Me) {
    val graph = LocalAppGraph.current
    val api = graph.session.api ?: return
    val vm: SettingsViewModel = viewModel { SettingsViewModel(ApiBackend(api)) }
    val prefs by vm.prefs.collectAsState()
    val sessions by vm.sessions.collectAsState()
    val server by vm.server.collectAsState()
    val message by vm.message.collectAsState()
    val quality by graph.devicePrefs.quality.collectAsState(initial = "auto")
    val scope = rememberCoroutineScope()
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 32.dp else 16.dp
    val memory = rememberFocusMemory()
    var open by remember { mutableStateOf<Open?>(null) }
    RestoreFocus(memory, ready = prefs !is Load.Loading)

    CompositionLocalProvider(LocalBringIntoViewSpec provides KeepInView) {
        LazyColumn(
            Modifier.fillMaxSize().testTag("settings"),
            contentPadding = PaddingValues(horizontal = edge, vertical = if (tv) 32.dp else 16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            item(key = "title") {
                Text("Settings", style = MaterialTheme.typography.headlineSmall.copy(fontWeight = FontWeight.ExtraBold), color = ReelColors.Ink)
            }
            message?.let { m -> item(key = "message") { Text(m, style = MaterialTheme.typography.bodyMedium, color = ReelColors.AccentHi, modifier = Modifier.padding(vertical = 8.dp).testTag("settings-message")) } }

            header("Account")
            item(key = "who") {
                SettingRow("Signed in as ${user.username}", api.server.origin, memory, "account", icon = Icons.Outlined.Dns, onClick = null)
            }
            item(key = "change-server") {
                SettingRow("Change server", "Connect to a different Reel server", memory, "change-server", onClick = { graph.session.changeServer() })
            }
            item(key = "sign-out") {
                SettingRow("Sign out", null, memory, "sign-out", icon = Icons.AutoMirrored.Outlined.Logout, onClick = { scope.launch { graph.session.signOut() } })
            }

            header("Playback")
            item(key = "quality") {
                SettingRow("Quality on this device", SettingsLogic.QUALITY_LABELS[quality] ?: quality, memory, "quality", onClick = { open = Open.Quality })
            }
            when (val p = prefs) {
                Load.Loading -> item(key = "prefs-loading") { Note("Loading your preferences…") }
                is Load.Failed -> item(key = "prefs-error") {
                    SettingRow("Couldn't load your preferences", "${p.message} · Try again", memory, "prefs-retry", onClick = { vm.loadPrefs() })
                }
                is Load.Ready -> prefsRows(p.value, memory, onOpen = { open = it }, onChange = vm::update)
            }

            header("Signed-in devices")
            when (val s = sessions) {
                Load.Loading -> item(key = "sessions-loading") { Note("Loading…") }
                is Load.Failed -> item(key = "sessions-error") {
                    SettingRow("Couldn't load devices", "${s.message} · Try again", memory, "sessions-retry", onClick = { vm.loadSessions() })
                }
                is Load.Ready -> {
                    val now = System.currentTimeMillis() / 1000
                    s.value.sortedWith(compareByDescending<SessionInfo> { it.current }.thenByDescending { it.lastSeen }).forEach { d ->
                        item(key = "session-${d.id}") {
                            val detail = listOfNotNull(
                                if (d.current) "This device" else null,
                                SettingsLogic.clientName(d.client),
                                d.ip,
                                if (d.current) null else SettingsLogic.lastSeen(d.lastSeen, now),
                            ).joinToString(" · ")
                            SettingRow(
                                d.deviceName ?: SettingsLogic.clientName(d.client) ?: "Unknown device",
                                detail,
                                memory,
                                "session-${d.id}",
                                trailing = if (d.current) null else "Sign out",
                                onClick = if (d.current) null else ({ open = Open.SignOutDevice(d) }),
                            )
                        }
                    }
                }
            }

            header("About")
            item(key = "about-app") {
                SettingRow("Reel for Android", "Version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})", memory, "about-app", onClick = null)
            }
            item(key = "about-server") {
                SettingRow("Server", serverVersion(server), memory, "about-server", onClick = null)
            }
        }
    }

    val p = (prefs as? Load.Ready)?.value
    when (val o = open) {
        null -> Unit
        Open.Quality -> ChoiceDialog(
            title = "Quality on this device",
            options = app.reel.android.data.DevicePrefs.QUALITIES,
            selected = quality,
            label = { SettingsLogic.QUALITY_LABELS[it] ?: it },
            onPick = {
                scope.launch { graph.devicePrefs.setQuality(it) }
                open = null
            },
            onDismiss = { open = null },
        )
        Open.SubtitleMode -> if (p != null) {
            ChoiceDialog(
                title = "Subtitles",
                options = SettingsLogic.SUBTITLE_MODES,
                selected = SettingsLogic.subtitleMode(p.subtitleMode),
                label = { it.label },
                onPick = { m ->
                    vm.update { it.copy(subtitleMode = m.id) }
                    open = null
                },
                onDismiss = { open = null },
            )
        }
        is Open.AddLanguage -> if (p != null) {
            val have = if (o.audio) p.audioLanguages else p.subtitleLanguages
            ChoiceDialog(
                title = if (o.audio) "Add an audio language" else "Add a subtitle language",
                options = SettingsLogic.addable(have, o.audio),
                selected = null,
                label = { SettingsLogic.languageName(it ?: "") },
                onPick = { code ->
                    if (code != null) vm.update { if (o.audio) it.copy(audioLanguages = it.audioLanguages + code) else it.copy(subtitleLanguages = it.subtitleLanguages + code) }
                    open = null
                },
                onDismiss = { open = null },
            )
        }
        is Open.EditLanguage -> if (p != null) {
            val list = if (o.audio) p.audioLanguages else p.subtitleLanguages
            val actions = buildList {
                if (o.index > 0) add("Move up")
                if (o.index < list.lastIndex) add("Move down")
                // Audio always keeps one language (the server falls back to *original* anyway).
                if (!o.audio || list.size > 1) add("Remove")
            }
            ChoiceDialog(
                title = list.getOrNull(o.index)?.let { SettingsLogic.languageName(it) } ?: "",
                options = actions,
                selected = null,
                label = { it ?: "" },
                onPick = { a ->
                    val next = when (a) {
                        "Move up" -> SettingsLogic.move(list, o.index, -1)
                        "Move down" -> SettingsLogic.move(list, o.index, 1)
                        "Remove" -> list.filterIndexed { i, _ -> i != o.index }
                        else -> list
                    }
                    vm.update { if (o.audio) it.copy(audioLanguages = next) else it.copy(subtitleLanguages = next) }
                    open = null
                },
                onDismiss = { open = null },
            )
        }
        is Open.SignOutDevice -> ChoiceDialog(
            title = "Sign out ${o.s.deviceName ?: "this device"}?",
            options = listOf("Sign out", "Cancel"),
            selected = "Cancel",
            label = { it },
            onPick = { a ->
                if (a == "Sign out") vm.revoke(o.s)
                open = null
            },
            onDismiss = { open = null },
        )
    }
}

private fun serverVersion(h: Health?): String = when {
    h == null -> "Checking…"
    h.version == null -> "Version not reported"
    else -> listOfNotNull("Version ${h.version}", h.gitSha?.take(7), h.buildDate).joinToString(" · ")
}

private fun LazyListScope.header(title: String) {
    item(key = "h-$title") {
        Text(
            title,
            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.ExtraBold),
            color = ReelColors.AccentHi,
            modifier = Modifier.padding(top = 24.dp, bottom = 8.dp),
        )
    }
}

private fun LazyListScope.prefsRows(p: PlaybackPrefs, memory: FocusMemory, onOpen: (Open) -> Unit, onChange: ((PlaybackPrefs) -> PlaybackPrefs) -> Unit) {
    item(key = "audio-h") { Note("Audio languages, in order of preference") }
    languageRows(p.audioLanguages, audio = true, memory, onOpen)
    val mode = SettingsLogic.subtitleMode(p.subtitleMode)
    item(key = "sub-mode") { SettingRow("Subtitles", "${mode.label} — ${mode.body}", memory, "sub-mode", onClick = { onOpen(Open.SubtitleMode) }) }
    item(key = "sub-h") { Note("Subtitle languages, in order of preference") }
    languageRows(p.subtitleLanguages, audio = false, memory, onOpen)
    item(key = "sdh") {
        ToggleRow("Prefer SDH subtitles", "Subtitles for the deaf and hard of hearing when a title has them", p.preferSdh, memory, "sdh") { v ->
            onChange { it.copy(preferSdh = v) }
        }
    }
    item(key = "burn") {
        ToggleRow("Allow image subtitles", "PGS / VobSub subtitles are burned into the picture (uses a transcode)", p.burnInImageSubs, memory, "burn") { v ->
            onChange { it.copy(burnInImageSubs = v) }
        }
    }
}

private fun LazyListScope.languageRows(list: List<String>, audio: Boolean, memory: FocusMemory, onOpen: (Open) -> Unit) {
    val kind = if (audio) "audio" else "sub"
    itemsIndexed(list, key = { i, code -> "$kind-$i-$code" }) { i, code ->
        SettingRow("${i + 1}. ${SettingsLogic.languageName(code)}", null, memory, "$kind-lang-$code", trailing = "Change", onClick = { onOpen(Open.EditLanguage(audio, i)) })
    }
    if (list.isEmpty()) item(key = "$kind-none") { Note("None yet: add one so subtitles can come on.") }
    item(key = "$kind-add") {
        SettingRow("Add a language", null, memory, "$kind-add", icon = Icons.Outlined.Add, onClick = { onOpen(Open.AddLanguage(audio)) })
    }
}

@Composable
private fun Note(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3, modifier = Modifier.padding(top = 8.dp, bottom = 4.dp))
}

/**
 * One settings row. Rows without [onClick] still take focus on TV, so D-pad can scroll to them
 * (About, the account line).
 */
@Composable
private fun SettingRow(
    title: String,
    detail: String?,
    memory: FocusMemory,
    key: String,
    icon: ImageVector? = null,
    trailing: String? = null,
    onClick: (() -> Unit)?,
    end: (@Composable (focused: Boolean) -> Unit)? = null,
) {
    FocusCard(
        onClick = { onClick?.invoke() },
        shape = RoundedCornerShape(10.dp),
        focusScale = ROW_FOCUS_SCALE,
        modifier = Modifier.fillMaxWidth().widthIn(max = 900.dp).focusMemory(memory, key).testTag("row-$key"),
    ) { focused ->
        Row(
            Modifier.fillMaxWidth().background(if (focused) ReelColors.Ink else ReelColors.Bg1).heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            val fg = if (focused) ReelColors.Bg else ReelColors.Ink
            if (icon != null) Icon(icon, contentDescription = null, tint = fg, modifier = Modifier.size(22.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold), color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (detail != null) Text(detail, style = MaterialTheme.typography.bodyMedium, color = if (focused) ReelColors.Bg.copy(alpha = 0.7f) else ReelColors.Ink2, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (trailing != null) Text(trailing, style = MaterialTheme.typography.labelLarge, color = if (focused) ReelColors.Bg else ReelColors.AccentHi)
            end?.invoke(focused)
        }
    }
}

@Composable
private fun ToggleRow(title: String, detail: String, checked: Boolean, memory: FocusMemory, key: String, onChange: (Boolean) -> Unit) {
    SettingRow(title, detail, memory, key, onClick = { onChange(!checked) }) { focused ->
        Box {
            Switch(
                checked = checked,
                onCheckedChange = null, // the whole row toggles, so OK on TV works
                colors = SwitchDefaults.colors(
                    checkedThumbColor = ReelColors.AccentInk,
                    checkedTrackColor = ReelColors.Accent,
                    uncheckedThumbColor = if (focused) ReelColors.Bg else ReelColors.Ink2,
                    uncheckedTrackColor = Color.Transparent,
                    uncheckedBorderColor = if (focused) ReelColors.Bg else ReelColors.Ink3,
                ),
                modifier = Modifier.testTag("switch-$key"),
            )
        }
    }
}
