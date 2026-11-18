# Add project specific ProGuard rules here.
# See https://developer.android.com/build/shrink-code

# JNA uses reflection and JNI; keep its runtime classes and native-mapped structures.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }

# UniFFI-generated bindings call into the native library by reflection; keep them.
-keep class uniffi.improv_android_ffi.** { *; }
