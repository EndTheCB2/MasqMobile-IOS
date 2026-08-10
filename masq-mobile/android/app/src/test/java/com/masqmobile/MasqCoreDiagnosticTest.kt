package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class MasqCoreDiagnosticTest {
  @Test
  fun treatsJsonNullLastErrorAsNoError() {
    assertEquals(null, safeLastErrorValue(null))
    assertEquals(null, safeLastErrorValue(Any()))
    assertEquals(null, safeLastErrorValue(""))
    assertEquals("E_TEST", safeLastErrorValue("E_TEST"))
  }

  @Test
  fun reportsOnlyBoundedConnectionStateAndExitCode() {
    val diagnostic =
        formatSafeCoreStatusDiagnostic(
            phase = "error",
            engineAvailable = true,
            connectedNeighbors = 0,
            routeStage = 0,
            proxyEnabled = false,
            lastError = "The embedded MASQ Node stopped with code 7. Check the Node log.",
        )

    assertEquals(
        "CORE_STATUS phase=error engine=true neighbors=0 route_stage=0 proxy=false error=exit_7",
        diagnostic,
    )
    assertFalse(diagnostic!!.contains("PRIVATE"))
  }

  @Test
  fun boundsUnexpectedStatusFieldsAndDoesNotIncludeFreeformErrors() {
    assertEquals(
        "CORE_STATUS phase=unknown engine=false neighbors=99 route_stage=0 proxy=true error=present",
        formatSafeCoreStatusDiagnostic(
            phase = "private phase",
            engineAvailable = false,
            connectedNeighbors = 10_000,
            routeStage = -4,
            proxyEnabled = true,
            lastError = "private diagnostic material",
        ),
    )
  }

  @Test
  fun reportsOnlyTheSafePanicFileTokenAndBoundedLine() {
    assertEquals(
        "CORE_STATUS phase=error engine=true neighbors=0 route_stage=0 proxy=false " +
            "error=panic_stream_handler_rs_123",
        formatSafeCoreStatusDiagnostic(
            phase = "error",
            engineAvailable = true,
            connectedNeighbors = 0,
            routeStage = 0,
            proxyEnabled = false,
            lastError = "E_CORE_PANIC_LOCATION: stream_handler.rs:123",
        ),
    )
  }
}
