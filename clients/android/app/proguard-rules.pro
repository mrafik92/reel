# App-specific R8 rules.

# API models (kotlinx.serialization, app.reel.android.data). The plugin generates a `$serializer`
# per @Serializable class and a `serializer()` on its companion; `serializer<T>()` finds them by
# reflection-free lookups that R8 can't see, so keep them. (The library ships its own core rules.)
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.**
-keep,includedescriptorclasses class app.reel.android.data.**$$serializer { *; }
-keepclassmembers class app.reel.android.data.** {
    *** Companion;
    kotlinx.serialization.KSerializer serializer(...);
}
-keepclasseswithmembers class app.reel.android.data.** {
    kotlinx.serialization.KSerializer serializer(...);
}
