use jni::{
    objects::{GlobalRef, JObject},
    JNIEnv,
};
use std::sync::OnceLock;

#[no_mangle]
pub extern "system" fn Java_blue_rae_spirit_sdk_AndroidNodeContext_install(
    mut env: JNIEnv,
    _receiver: JObject,
    context: JObject,
) {
    static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();
    let result = (|| -> jni::errors::Result<()> {
        let vm = env.get_java_vm()?;
        let global = env.new_global_ref(context)?;
        CONTEXT.get_or_init(|| {
            unsafe {
                iroh_dns::install_android_jni_context(
                    vm.get_java_vm_pointer().cast(),
                    global.as_obj().as_raw().cast(),
                );
            }
            global
        });
        Ok(())
    })();
    if let Err(error) = result {
        let _ = env.throw_new("java/lang/IllegalStateException", error.to_string());
    }
}
