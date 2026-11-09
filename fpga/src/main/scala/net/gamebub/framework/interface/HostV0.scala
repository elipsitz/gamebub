package net.gamebub.framework.interface

import chisel3._

object HostV0 {
    val CommandGetStatus = 0x0000
    val CommandCoreRun = 0x0100
    val CommandCoreHalt = 0x0101
    val CommandSetupComplete = 0x0102
    val CommandNotifyFocus = 0x0200
    val CommandFileWriteStart = 0x0300
    val CommandFileWriteEnd = 0x0301
    val CommandFileReadStart = 0x0302
    val CommandFileReadEnd = 0x0303
    val CommandFileGetSize = 0x0400
    val CommandFileSetSize = 0x0401
    val CommandFileRead = 0x0402
    val CommandFileWrite = 0x0403
    val CommandFileFlush = 0x0404

    val FileStatusOk = 0
    val FileStatusErrInvalidFile = 1
    val FileStatusErrIo = 2
    val FileStatusErrBounds = 3

    class CommandChannel extends Bundle {
        /** Whether a request is active: held high for the duration of the request. */
        val request = Input(Bool())
        /** High when the target acknowledges the command. */
        val busy = Output(Bool())
        /** High when the command is completed. */
        val done = Output(Bool())
        /** If high, indicates that the command completed with an error. */
        val error = Output(Bool())
    }

    class MemoryInterface extends Bundle {
        /** Access enable */
        val enable = Input(Bool())
        /** Whether the access is a write */
        val write = Input(Bool())
        /** True when the access is complete. */
        val done = Output(Bool())
        /** Access address */
        val address = Input(UInt(32.W))
        /** Read data */
        val dataRead = Output(UInt(32.W))
        /** Write data */
        val dataWrite = Input(UInt(32.W))
    }

    /** Unknown status */
    val StatusUnknown = 0
    /** Initializing, preparing peripherals */
    val StatusInitialize = 1
    /** Ready for set up, data and register access. */
    val StatusSetup = 2
    /** The core is set up, but in a halted/reset state. */
    val StatusCoreHalt = 3
    /** The core is running. */
    val StatusCoreRun = 4
}

class HostV0 extends Bundle {
    val mem = new HostV0.MemoryInterface

    /** Command channel for Host -> Core commands **/
    val commandHost = new HostV0.CommandChannel
    /** Command channel for Core -> Host commands **/
    val commandCore = Flipped(new HostV0.CommandChannel)
}
