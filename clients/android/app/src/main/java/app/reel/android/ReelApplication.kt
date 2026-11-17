package app.reel.android

import android.app.Application
import coil.ImageLoader
import coil.ImageLoaderFactory

class ReelApplication : Application(), ImageLoaderFactory {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        graph = AppGraph(this)
    }

    /** Coil shares the app's OkHttp pool but not ReelApi's token interceptor: image URLs are signed. */
    override fun newImageLoader(): ImageLoader = ImageLoader.Builder(this)
        .okHttpClient(graph.http)
        .crossfade(true)
        .build()
}
