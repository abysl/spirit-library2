plugins {
    `kotlin-dsl`
}

repositories {
    mavenCentral()
    gradlePluginPortal()
}

kotlin {
    jvmToolchain(25)
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21)
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

dependencies {
    testImplementation(kotlin("test"))
}

gradlePlugin {
    plugins {
        create("spiritNativePreparation") {
            id = "spirit.native-preparation"
            implementationClass = "blue.rae.spirit.nativeprep.SpiritNativePreparationPlugin"
        }
    }
}

tasks.test {
    useJUnitPlatform()
}
