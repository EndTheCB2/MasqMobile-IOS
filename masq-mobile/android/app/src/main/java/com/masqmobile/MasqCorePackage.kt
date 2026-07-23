package com.masqmobile

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class MasqCorePackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
      if (name == MasqCoreModule.NAME) MasqCoreModule(reactContext) else null

  override fun getReactModuleInfoProvider() =
      ReactModuleInfoProvider {
        mapOf(
            MasqCoreModule.NAME to
                ReactModuleInfo(
                    MasqCoreModule.NAME,
                    MasqCoreModule::class.java.name,
                    false,
                    false,
                    false,
                    true,
                )
        )
      }
}
