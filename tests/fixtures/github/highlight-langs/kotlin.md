```kotlin
package demo

data class User(val name: String, var age: Int = 0)

fun main(args: Array<String>) {
    val users = listOf(User("a"), User("b", 3))
    users.filter { it.age > 1 }.forEach { println("${it.name} is $it") }
    when (val x = args.size) {
        0 -> println("none")
        else -> throw IllegalStateException()
    }
}
```
