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
# Tauri 移动端插件命令通过反射调用（commands[method.name]），
# release 开启 minify 后必须保留注解成员，否则会报 "No command xxx found for plugin"。
-keepclassmembers class * {
    @app.tauri.annotation.Command <methods>;
}
-keepclassmembers class * {
    @app.tauri.annotation.InvokeArg class *;
}
-keep class app.tauri.annotation.** { *; }
-keep class com.plugin.vpnservice.** { *; }
