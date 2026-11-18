package app.reel.android.ui.home

import androidx.lifecycle.viewModelScope
import app.reel.android.data.ReelApi
import app.reel.android.data.ServerAddress
import app.reel.android.ui.common.Load
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class HomeViewModelTest {
    @Test
    fun refreshKeepsLoadedCatalogueWhenRequestFails() = runBlocking {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        val server = MockWebServer()
        server.start()
        val api = ReelApi(ServerAddress.parse(server.url("/").toString())!!, OkHttpClient(), token = { "t" })
        server.enqueue(MockResponse().setBody("""{"continue_watching":[],"recently_added":[],"rows":[]}"""))
        val vm = HomeViewModel(api)
        try {
            withTimeout(5000) { vm.state.first { it is Load.Ready } }
            server.enqueue(MockResponse().setResponseCode(500))
            vm.refresh()
            server.takeRequest()
            server.takeRequest()
            // Drain the in-flight request before checking the retained state.
            vm.viewModelScope.coroutineContext[Job]!!.children.forEach { it.join() }
            assertTrue(vm.state.value is Load.Ready)
        } finally {
            vm.viewModelScope.coroutineContext[Job]?.cancelAndJoin()
            server.shutdown()
            Dispatchers.resetMain()
        }
    }
}
