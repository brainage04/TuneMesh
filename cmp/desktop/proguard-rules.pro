# JNA maps the libmpv interface and structures by reflection.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class io.github.brainage04.tunemesh.desktop.playback.LibMpv { *; }
-keep class io.github.brainage04.tunemesh.desktop.playback.LibMpv$* { *; }
-dontwarn com.sun.jna.**

# jaudiotagger picks its readers reflectively and references optional Android/ImageIO classes.
-keep class org.jaudiotagger.** { *; }
-dontwarn org.jaudiotagger.**

# kotlinx.serialization
-keepattributes *Annotation*, InnerClasses, Signature
-keepclassmembers class **$$serializer { *; }
-keepclassmembers @kotlinx.serialization.Serializable class ** {
    static ** Companion;
    *** Companion;
    kotlinx.serialization.KSerializer serializer(...);
}
-keep class **$$serializer { *; }
-dontwarn kotlinx.serialization.**

-dontwarn kotlinx.coroutines.debug.**

# Held only to keep jaudiotagger's logger (and its OFF level) alive.
-keepclassmembers class io.github.brainage04.tunemesh.desktop.data.TagReader { private ** quietLogger; }
