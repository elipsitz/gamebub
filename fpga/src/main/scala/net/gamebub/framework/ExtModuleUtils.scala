package net.gamebub.framework

import chisel3._
import chisel3.reflect.DataMirror
import scala.annotation.nowarn

object ExtModuleUtils {
  def bindExtModule[T <: Bundle](
    extModuleName: String,
    io: T,
    params: Map[String, Param],
  ) = {
    val ioType = chiselTypeOf(io)
    val extModule = Module(new ExtModule(params) {
      override val desiredName: String = extModuleName

      val clock = FlatIO(Input(Clock()))
      val reset = FlatIO(Input(Reset()))
      val io: T = FlatIO(ioType)
    })

    extModule.clock := Module.clock
    extModule.reset := Module.reset
    extModule.io <> io

    println(ExtModuleUtils.generateVerilogTemplate(extModule))
  }

  def generateVerilogTemplate(module: ExtModule): String = {
    def inner(data: Data, prefix: String): Seq[String] = {
      data match {
        case b: Bundle => {
          b.elements.toSeq.reverse.flatMap { case (name, element) =>
            inner(element, if (prefix.isEmpty) name else s"${prefix}_$name")
          }
        }

        case v: Vec[_] => {
          v.zipWithIndex.flatMap { case (element, index) =>
            inner(element, if (prefix.isEmpty) s"$index" else s"${prefix}_$index")
          }
        }
          
        case e: Element => {
          val direction = DataMirror.directionOf(e) match {
            case ActualDirection.Input  => "input "
            case ActualDirection.Output => "output"
            case _                      => ???
          }

          val typeStr = e match {
            case _: SInt => "logic signed"
            case _       => "logic       "
          }
          
          val width = e.widthOption.getOrElse(1)
          val widthStr = if (width > 1) s"[${width - 1}:0]" else ""
          Seq(f"  $direction $typeStr $widthStr%-6s $prefix")
        }
      }
    }

    val moduleName = module.name
    val ports = DataMirror.modulePorts(module)
    val portLines = ports.flatMap { case (name, data) => {
      inner(data, prefix = name)
    }}
    @nowarn("cat=deprecation")
    val paramLines = module.params.map { case (name, param) => {
      val typeStr = param match {
        case p: IntParam => "int "
        case p: DoubleParam => "real"
        // TODO add more param types
        case _ => "    "
      }
      f"  parameter $typeStr $name"
    }}
    
    s"""|module ${moduleName}
        |#(
        |${paramLines.mkString(",\n")}
        |)
        |(
        |${portLines.mkString(",\n")}
        |);
        |
        |  // Add module implementation here.
        |
        |endmodule
        |""".stripMargin
  }
}