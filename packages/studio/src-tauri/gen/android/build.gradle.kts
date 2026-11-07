buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.3.2")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.1.21")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
        // rustls 的 Android 平台证书校验 AAR（注意 group 是 org.rustls）。
        // 必须用 raw.githubusercontent：github.com/.../raw/ 会 302，gradle 解析不到。
        maven {
            url = uri("https://raw.githubusercontent.com/rustls/rustls-platform-verifier/maven-archive/android-release-support/maven/")
        }
    }
}

tasks.register("clean").configure {
    delete("build")
}
