package app.reel.android.ui.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.reel.android.data.Home
import app.reel.android.data.ReelApi
import app.reel.android.data.home
import app.reel.android.ui.common.Load
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Keeps the visible catalogue while returning from another destination or refreshing progress. */
class HomeViewModel(private val api: ReelApi) : ViewModel() {
    private val mutableState = MutableStateFlow<Load<Home>>(Load.Loading)
    val state = mutableState.asStateFlow()
    private var request: Job? = null

    init { refresh() }

    fun refresh() {
        if (request?.isActive == true) return
        request = viewModelScope.launch {
            try {
                mutableState.value = Load.Ready(api.home())
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (mutableState.value !is Load.Ready) mutableState.value = Load.Failed(e)
            }
        }
    }
}
