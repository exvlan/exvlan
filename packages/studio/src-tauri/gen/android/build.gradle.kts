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
        // rustls-platform-verifier 的 Android 原生校验组件（Kotlin/AAR）
    }
}

tasks.register("clean").configure {
    delete("build")
}
