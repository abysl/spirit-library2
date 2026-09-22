package blue.rae.spirit.nativeprep

import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.tasks.Exec
import org.gradle.api.tasks.bundling.Compression
import org.gradle.api.tasks.bundling.Tar
import org.gradle.kotlin.dsl.register

class SpiritNativePreparationPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        check(project == project.rootProject) { "spirit.native-preparation must be applied to the root project" }

        val repositoryRoot = project.rootDir.parentFile
        val rustDirectory = repositoryRoot.resolve("rust")
        val releaseDirectory = rustDirectory.resolve("target/release")
        val bindingsDirectory = repositoryRoot.resolve("kmp/sdk/src/commonMain/kotlin")
        val generatedBindingsDirectory = bindingsDirectory.resolve("uniffi")
        val systemName = System.getProperty("os.name")
        val nativeLibrary = releaseDirectory.resolve(NativeArtifacts.sharedLibraryName(systemName))
        val bindgen = releaseDirectory.resolve(NativeArtifacts.uniffiBindgenName(systemName))
        val cli = releaseDirectory.resolve(NativeArtifacts.executableName(systemName))
        val archiveTarget = project.providers.gradleProperty("archiveTarget")

        val buildNative = project.tasks.register<Exec>("buildNative") {
            group = "build"
            description = "Build the Spirit FFI library, CLI, and UniFFI bindgen"
            workingDir = rustDirectory
            commandLine(
                "cargo", "build", "--locked", "--release",
                "-p", "spirit-ffi", "-p", "spirit-cli", "--features", "uniffi/cli",
            )
            inputs.files(project.fileTree(rustDirectory) {
                exclude("target/**")
            })
            outputs.files(nativeLibrary, bindgen, cli)
            outputs.upToDateWhen { false }
        }

        val generateKotlinBindings = project.tasks.register<Exec>("generateKotlinBindings") {
            group = "build"
            description = "Generate Kotlin bindings from the built Spirit FFI library"
            dependsOn(buildNative)
            workingDir = rustDirectory
            commandLine(
                bindgen.absolutePath,
                "generate",
                "--library", nativeLibrary.absolutePath,
                "--language", "kotlin",
                "--out-dir", bindingsDirectory.absolutePath,
                "--no-format",
            )
            inputs.files(nativeLibrary, bindgen)
            outputs.dir(generatedBindingsDirectory)
        }

        val archiveCli = project.tasks.register<Tar>("archiveCli") {
            group = "distribution"
            description = "Archive the host Spirit CLI for the requested archiveTarget"
            dependsOn(buildNative)
            onlyIf { archiveTarget.isPresent }
            compression = Compression.GZIP
            destinationDirectory.set(repositoryRoot.resolve("dist"))
            archiveFileName.set(archiveTarget.map(NativeArtifacts::archiveFileName))
            from(cli) {
                filePermissions { unix("755") }
            }
            inputs.file(cli)
        }

        project.tasks.register("prepareNative") {
            group = "build"
            description = "Build native artifacts and generate Kotlin bindings"
            dependsOn(generateKotlinBindings)
            if (archiveTarget.isPresent) {
                dependsOn(archiveCli)
            }
        }
    }
}
