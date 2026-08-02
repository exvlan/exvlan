import java.io.File
import javax.inject.Inject
import org.apache.tools.ant.taskdefs.condition.Os
import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.ProjectLayout
import org.gradle.api.logging.LogLevel
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

abstract class BuildTask @Inject constructor(
    private val execOps: ExecOperations,
    private val layout: ProjectLayout
) : DefaultTask() {
    @Input
    var rootDirRel: String? = null
    @Input
    var target: String? = null
    @Input
    var release: Boolean? = null

    @TaskAction
    fun assemble() {
        val executable = """pnpm""";
        try {
            runTauriCli(executable)
        } catch (e: Exception) {
            if (Os.isFamily(Os.FAMILY_WINDOWS)) {
                runTauriCli("$executable.cmd")
            } else {
                throw e;
            }
        }
    }

    fun runTauriCli(executable: String) {
        val rootDirRel = rootDirRel ?: throw GradleException("rootDirRel cannot be null")
        val target = target ?: throw GradleException("target cannot be null")
        val release = release ?: throw GradleException("release cannot be null")

        execOps.exec {
            workingDir = layout.projectDirectory.dir(rootDirRel).asFile
            commandLine(buildList {
                add(executable)
                add("tauri")
                add("android")
                add("android-studio-script")
                if (logger.isEnabled(LogLevel.DEBUG)) {
                    add("-vv")
                } else if (logger.isEnabled(LogLevel.INFO)) {
                    add("-v")
                }
                if (release) {
                    add("--release")
                }
                add("--target")
                add(target)
            })
        }.assertNormalExitValue()
    }
}