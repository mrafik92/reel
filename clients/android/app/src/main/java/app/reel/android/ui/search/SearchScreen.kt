package app.reel.android.ui.search

import android.app.Activity
import android.content.Intent
import android.speech.RecognizerIntent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import app.reel.android.DeviceKind
import app.reel.android.LocalAppGraph
import app.reel.android.LocalDeviceKind
import app.reel.android.data.ItemSummary
import app.reel.android.data.PersonHit
import app.reel.android.data.SearchAll
import app.reel.android.data.searchAll
import app.reel.android.ui.common.EmptyView
import app.reel.android.ui.common.ErrorView
import app.reel.android.ui.common.LandscapeCard
import app.reel.android.ui.common.Load
import app.reel.android.ui.common.LoadingView
import app.reel.android.ui.common.PosterCard
import app.reel.android.ui.common.ReelImage
import app.reel.android.ui.item.ItemLogic
import app.reel.android.ui.shell.LocalNavController
import app.reel.android.ui.shell.Routes
import app.reel.android.ui.theme.ReelColors
import app.reel.android.ui.tv.FocusCard
import app.reel.android.ui.tv.FocusMemory
import app.reel.android.ui.tv.RestoreFocus
import app.reel.android.ui.tv.focusMemory
import app.reel.android.ui.tv.rememberFocusMemory

/**
 * Search (#189): one box over `GET /search/all`, results as rows (titles, episodes, people).
 *
 * - TV: the box takes focus and the on-screen keyboard opens at once; its search key (or D-pad
 *   down) moves to the results. Back from a result returns to that result.
 * - Voice: a mic button when the device has a speech recognizer (most Android TVs, also from the
 *   remote's mic through the keyboard).
 * - A person opens a search for their name (the library part lists their titles).
 */
@Composable
fun SearchScreen() {
    val api = LocalAppGraph.current.session.api ?: return
    val vm: SearchViewModel = viewModel { SearchViewModel { q -> api.searchAll(q) } }
    val query by vm.query.collectAsState()
    val state by vm.state.collectAsState()
    val nav = LocalNavController.current
    SearchContent(
        query = query,
        state = state,
        onQuery = vm::setQuery,
        onSubmit = { vm.submit() },
        onRetry = { vm.submit(force = true) },
        onOpen = { nav.navigate(Routes.item(it.id)) },
        onPerson = {
            vm.setQuery(it.name)
            vm.submit()
        },
    )
}

/** The stateless search page; [SearchScreen] feeds it from [SearchViewModel]. */
@Composable
fun SearchContent(
    query: String,
    state: SearchState,
    onQuery: (String) -> Unit,
    onSubmit: () -> Unit,
    onRetry: () -> Unit,
    onOpen: (ItemSummary) -> Unit,
    onPerson: (PersonHit) -> Unit,
) {
    val tv = LocalDeviceKind.current == DeviceKind.Tv
    val edge = if (tv) 32.dp else 16.dp
    val memory = rememberFocusMemory()
    val keyboard = LocalSoftwareKeyboardController.current
    val focus = LocalFocusManager.current
    val firstVisit = remember { memory.lastKey == null || memory.lastKey == FIELD }

    RestoreFocus(memory, first = FIELD)
    if (tv && firstVisit) {
        // The keyboard opens straight away on entry (not when coming back from a result).
        LaunchedEffect(Unit) {
            repeat(10) {
                kotlinx.coroutines.delay(50)
                if (memory.lastKey == FIELD) {
                    keyboard?.show()
                    return@LaunchedEffect
                }
            }
        }
    }

    // Leaving the box goes to the first result (not whichever poster is under the box's centre).
    val firstResult = remember { FocusRequester() }
    val goDown: () -> Boolean = {
        keyboard?.hide()
        runCatching { firstResult.requestFocus() }.isSuccess || focus.moveFocus(FocusDirection.Down)
    }
    val toResults = {
        onSubmit()
        goDown()
        Unit
    }

    Column(Modifier.fillMaxSize().testTag("search")) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = edge, vertical = if (tv) 24.dp else 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // Text set from outside (a person, voice, Clear) puts the cursor at the end.
            var field by remember { mutableStateOf(TextFieldValue(query, TextRange(query.length))) }
            if (field.text != query) field = TextFieldValue(query, TextRange(query.length))
            OutlinedTextField(
                value = field,
                onValueChange = {
                    field = it
                    if (it.text != query) onQuery(it.text)
                },
                singleLine = true,
                placeholder = { Text("Titles, actors, episodes", color = ReelColors.Ink3) },
                leadingIcon = { Icon(Icons.Outlined.Search, contentDescription = null, tint = ReelColors.Ink2) },
                trailingIcon = if (query.isNotEmpty()) {
                    { IconButton(onClick = { onQuery("") }) { Icon(Icons.Outlined.Close, contentDescription = "Clear", tint = ReelColors.Ink2) } }
                } else {
                    null
                },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSearch = { toResults() }),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = ReelColors.Ink,
                    unfocusedBorderColor = ReelColors.Ink3,
                    cursorColor = ReelColors.AccentHi,
                    focusedTextColor = ReelColors.Ink,
                    unfocusedTextColor = ReelColors.Ink,
                ),
                modifier = Modifier
                    .weight(1f)
                    .focusMemory(memory, FIELD)
                    // A text field keeps the D-pad for its cursor; Down leaves it for the results.
                    .onPreviewKeyEvent { e ->
                        if (e.type == KeyEventType.KeyDown && e.key == Key.DirectionDown) {
                            goDown()
                        } else {
                            false
                        }
                    }
                    .testTag("search-field"),
            )
            VoiceButton(onText = {
                onQuery(it)
                onSubmit()
            }, memory = memory)
        }
        when (state) {
            SearchState.Idle -> EmptyView(if (tv) "Type with the remote, or use the mic." else "Search your library: titles, actors, episodes.")
            is SearchState.Loading -> LoadingView()
            is SearchState.Failed -> ErrorView(Load.Failed(state.error).message, onRetry = onRetry)
            is SearchState.Results -> if (state.results.isEmpty) {
                EmptyView("Nothing matches “${state.q}”.")
            } else {
                Results(state.results, memory, firstResult, edge, tv, onOpen, onPerson)
            }
        }
    }
}

private const val FIELD = "field"

/** The mic, when the device can turn speech into text. */
@Composable
private fun VoiceButton(onText: (String) -> Unit, memory: FocusMemory) {
    val context = LocalContext.current
    val intent = remember {
        Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_PROMPT, "Search Reel")
    }
    val available = remember { intent.resolveActivity(context.packageManager) != null }
    if (!available) return
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
        if (r.resultCode == Activity.RESULT_OK) {
            r.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)?.firstOrNull()?.takeIf { it.isNotBlank() }?.let(onText)
        }
    }
    FocusCard(
        onClick = { runCatching { launcher.launch(intent) } },
        shape = CircleShape,
        modifier = Modifier.padding(start = 12.dp).size(52.dp).focusMemory(memory, "mic").testTag("voice"),
    ) { focused ->
        Box(Modifier.fillMaxSize().background(if (focused) ReelColors.Ink else ReelColors.Bg2), contentAlignment = Alignment.Center) {
            Icon(Icons.Outlined.Mic, contentDescription = "Search by voice", tint = if (focused) ReelColors.Bg else ReelColors.Ink)
        }
    }
}

@Composable
private fun Results(
    r: SearchAll,
    memory: FocusMemory,
    first: FocusRequester,
    edge: androidx.compose.ui.unit.Dp,
    tv: Boolean,
    onOpen: (ItemSummary) -> Unit,
    onPerson: (PersonHit) -> Unit,
) {
    // The first card of the first row also answers to [first].
    val firstKey = r.library.firstOrNull()?.let { "title/${it.id}" }
        ?: r.episodes.firstOrNull()?.let { "episode/${it.id}" }
        ?: r.people.firstOrNull()?.let { "person/${it.tmdbId}" }
    fun Modifier.tag(key: String) = focusMemory(memory, key).then(if (key == firstKey) Modifier.focusRequester(first) else Modifier)
    LazyColumn(
        Modifier.fillMaxSize().testTag("search-results"),
        contentPadding = PaddingValues(bottom = 32.dp),
        verticalArrangement = Arrangement.spacedBy(if (tv) 24.dp else 20.dp),
    ) {
        if (r.library.isNotEmpty()) {
            item(key = "titles") {
                ResultRow(title = "Titles", edge = edge) {
                    items(r.library, key = { "t-${it.id}" }) { i ->
                        PosterCard(
                            i.title,
                            i.posterUrl,
                            onClick = { onOpen(i) },
                            modifier = Modifier.tag("title/${i.id}").testTag("result-${i.id}"),
                            subtitle = listOfNotNull(i.year?.toString(), if (i.kind == "show") "Series" else null).joinToString(" · ").ifEmpty { null },
                            progress = i.progress,
                            played = i.played,
                        )
                    }
                }
            }
        }
        if (r.episodes.isNotEmpty()) {
            item(key = "episodes") {
                ResultRow(title = "Episodes", edge = edge) {
                    items(r.episodes, key = { "e-${it.id}" }) { e ->
                        LandscapeCard(
                            e.title,
                            e.backdropUrl ?: e.posterUrl,
                            onClick = { onOpen(e) },
                            modifier = Modifier.tag("episode/${e.id}").testTag("result-${e.id}"),
                            subtitle = ItemLogic.episodeCode(e.seasonNo, e.episodeNo),
                            progress = e.progress,
                            played = e.played,
                        )
                    }
                }
            }
        }
        if (r.people.isNotEmpty()) {
            item(key = "people") {
                ResultRow(title = "People", edge = edge) {
                    items(r.people, key = { "p-${it.tmdbId}-${it.name}" }) { p ->
                        PersonCard(p, Modifier.tag("person/${p.tmdbId}").testTag("person-${p.tmdbId}")) { onPerson(p) }
                    }
                }
            }
        }
    }
}

/** A titled horizontal row of results. */
@Composable
private fun ResultRow(title: String, edge: androidx.compose.ui.unit.Dp, content: androidx.compose.foundation.lazy.LazyListScope.() -> Unit) {
    Column {
        Text(
            title,
            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.ExtraBold),
            color = ReelColors.Ink,
            modifier = Modifier.padding(start = edge, bottom = 8.dp),
        )
        LazyRow(
            state = rememberLazyListState(),
            contentPadding = PaddingValues(horizontal = edge, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            content = content,
        )
    }
}

@Composable
private fun PersonCard(p: PersonHit, modifier: Modifier, onClick: () -> Unit) {
    Column(modifier.width(120.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        FocusCard(onClick = onClick, shape = CircleShape, modifier = Modifier.size(96.dp)) {
            ReelImage(p.photoUrl, contentDescription = p.name, modifier = Modifier.fillMaxSize())
        }
        Text(p.name, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 8.dp))
        val sub = if (p.inLibraryCount > 0) "${p.inLibraryCount} in your library" else p.knownForDepartment
        sub?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = ReelColors.Ink3, maxLines = 1, overflow = TextOverflow.Ellipsis) }
    }
}
