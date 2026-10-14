import java.util.Properties
import java.io.FileInputStream
import java.security.KeyStore
import groovy.json.JsonSlurper
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val versionPattern = Regex("""^(\d+)\.(\d+)\.(\d+)$""")

val tauriVersionName = tauriProperties.getProperty("tauri.android.versionName")?.ifBlank { null } ?: run {
    val tauriConfFile = file("../../../tauri.conf.json")
    check(tauriConfFile.exists()) { "Missing tauri.conf.json at ${tauriConfFile.path}" }

    val tauriConf = tauriConfFile.reader(Charsets.UTF_8).use { JsonSlurper().parse(it) as? Map<*, *> }
        ?: error("Failed to parse ${tauriConfFile.path} as a JSON object")
    tauriConf["version"] as? String
        ?: error("Missing string field \"version\" in ${tauriConfFile.path}")
}

val tauriVersionMatch = versionPattern.matchEntire(tauriVersionName)
    ?: error("Android version must use x.y.z format, but got \"$tauriVersionName\"")

val tauriVersionCode = if (tauriProperties.getProperty("tauri.android.versionName")?.ifBlank { null } != null) {
    val versionCodeProp = tauriProperties.getProperty("tauri.android.versionCode")
    if (versionCodeProp != null) {
        versionCodeProp.toIntOrNull()
            ?: error("Property \"tauri.android.versionCode\" must be an integer, but got \"$versionCodeProp\"")
    } else {
        val (major, minor, patch) = tauriVersionMatch.destructured
        major.toInt() * 1_000_000 + minor.toInt() * 1_000 + patch.toInt()
    }
} else {
    val (major, minor, patch) = tauriVersionMatch.destructured
    major.toInt() * 1_000_000 + minor.toInt() * 1_000 + patch.toInt()
}

android {
    compileSdk = 36
    namespace = "era.exvlan.studio"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "era.exvlan.studio"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriVersionCode
        versionName = tauriVersionName
    }
    signingConfigs {
        create("release") {
            // 1. 尝试从环境变量读取
            val envKeyAlias = System.getenv("KEYSTORE_ALIAS")
            val envKeyPassword = System.getenv("KEYSTORE_PASSWORD")
            val envStoreFile = System.getenv("KEYSTORE_PATH")
            val envStorePassword = System.getenv("KEYSTORE_PASSWORD") // 通常 store 和 key 密码相同

            // 2. 读取配置文件作为备选
            val keystorePropertiesFile = rootProject.file("keystore.properties")
            val keystoreProperties = Properties()
            if (keystorePropertiesFile.exists()) {
                keystoreProperties.load(FileInputStream(keystorePropertiesFile))
            }

            // 3. 优先级逻辑：环境变量 > 属性文件 > 默认值/空
            keyAlias = envKeyAlias ?: (keystoreProperties["keyAlias"] as? String) ?: ""
            keyPassword = envKeyPassword ?: (keystoreProperties["keyPassword"] as? String) ?: ""
            storePassword = envStorePassword ?: (keystoreProperties["storePassword"] as? String) ?: ""
            
            val filePath = envStoreFile ?: (keystoreProperties["storeFile"] as? String)
            if (!filePath.isNullOrEmpty()) {
                storeFile = file(filePath)
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
            // 仅当 release keystore 实际可加载时使用它（真实发布流水线通过 KEYSTORE_* 提供）；
            // 否则回退到 debug 签名，保证 CI 能产出可安装 APK 而不因 keystore 密码不匹配而失败。
            val releaseSigning = signingConfigs.getByName("release")
            val hasUsableKeystore = run {
                val store = releaseSigning.storeFile
                val pw = releaseSigning.storePassword
                if (store != null && store.exists() && !pw.isNullOrEmpty()) {
                    try {
                        KeyStore.getInstance(store, pw.toCharArray()).load(null, null)
                        true
                    } catch (e: Throwable) {
                        logger.warn("Release keystore unusable (${store.path}): ${e.message}; falling back to debug signing")
                        false
                    }
                } else {
                    false
                }
            }
            signingConfig = if (hasUsableKeystore) releaseSigning else signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.6.1")
    implementation("androidx.core:core:1.18.0")
    implementation("androidx.core:core-ktx:1.18.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.13.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_21
    }
}

apply(from = "tauri.build.gradle.kts")
