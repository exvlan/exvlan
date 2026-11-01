package era.exvlan.studio

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.util.TypedValue
import android.view.View
import androidx.activity.OnBackPressedCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Android 15+（targetSdk 36）强制 edge-to-edge：内容会画到状态栏/手势条下方，
        // 这里让内容视图自己承担安全区 padding。
        WindowCompat.setDecorFitsSystemWindows(window, false)
        applySafeAreaInsets()

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                moveTaskToBack(true)
            }
        })
        initService()
    }

    /**
     * 给内容视图加系统栏 padding：
     * - systemBars + displayCutout：状态栏、三键/手势导航栏、刘海
     * - ime：软键盘（与 systemBars 底部取 max，避免 adjustResize 造成双倍留白）
     * 背景色取主题的 colorBackground，跟随深浅色主题。
     */
    private fun applySafeAreaInsets() {
        val content = findViewById<View>(android.R.id.content)

        val tv = TypedValue()
        if (theme.resolveAttribute(android.R.attr.colorBackground, tv, true)) {
            content.setBackgroundColor(tv.data)
        }

        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
            )
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            view.setPadding(
                bars.left,
                bars.top,
                bars.right,
                maxOf(bars.bottom, ime.bottom)
            )
            insets
        }
        ViewCompat.requestApplyInsets(content)
    }

    private fun initService() {
        val serviceIntent = Intent(this, MainForegroundService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(serviceIntent)
        } else {
            startService(serviceIntent)
        }
    }
}
