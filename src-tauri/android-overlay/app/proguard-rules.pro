# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# btleplug's droidplug Java companion is accessed solely via JNI, so the
# optimizer can't see the references and would strip the classes under
# R8/ProGuard minification. Keep them (and their jni-utils-rs deps) as-is.
-keep class com.nonpolynomial.** { *; }
-keep class io.github.gedgygedgy.** { *; }

# Our own LedgerBleBridge exposes native methods called from Rust via JNI;
# keep the class and its members so R8 doesn't obfuscate the symbol names.
-keep class org.fistbump.wallet.LedgerBleBridge { *; }
