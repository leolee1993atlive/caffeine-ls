# Regenerating `bin/`

`src/demo/*.java` is the project as it exists **before** annotation processing.
`bin/demo/*.class` is the bytecode an annotation processor (Lombok's
`@Data`/`@Slf4j`/`@Builder`, simulated by hand here) would generate from it.
The fixture is completely offline: no Lombok jar is involved.

The integration tests copy the fixture into a temporary directory and set the
`src/**` and `bin/**` mtimes themselves (see
`crates/caffeine-ls/tests/mirror_fallback.rs`), so checkout mtimes do not
matter. Do not rely on them.

## Expanded sources

Write these two files into a temporary directory (both `package demo;`):

`Person.java`

```java
package demo;

public class Person {
    private String name;
    private int age;
    private static final String log = "log";

    public Person() {
    }

    public Person(String name, int age) {
        this.name = name;
        this.age = age;
    }

    public static Person of(String name) {
        Person person = new Person();
        person.name = name;
        return person;
    }

    public String getName() {
        return name;
    }

    public void setName(String name) {
        this.name = name;
    }

    public int getAge() {
        return age;
    }

    public String mirrorField() {
        return log;
    }

    public static PersonBuilder builder() {
        return new PersonBuilder();
    }

    public static class PersonBuilder {
        private String name;

        public PersonBuilder name(String name) {
            this.name = name;
            return this;
        }

        public Person build() {
            Person person = new Person();
            person.name = name;
            return person;
        }
    }
}
```

`Main.java`

```java
package demo;

public class Main {
    public static void main(String[] args) {
        Person p = Person.of("alice");
        System.out.println(p.getName());
        Person q = new Person("bob", 2);
        Person.PersonBuilder b = Person.builder();
        Person r = b.name("carol").build();
        System.out.println(p.getAge());
        System.out.println(r.mirrorField());
    }
}
```

## Commands (PowerShell)

```powershell
$bin = "crates/caffeine-ls/tests/fixtures/lombok-mirror/bin"
Remove-Item -Recurse -Force $bin -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $bin | Out-Null
& C:/Users/limy/.jdks/dragonwell-21.0.11/bin/javac.exe -d $bin <tmp>/Person.java <tmp>/Main.java
& C:/Users/limy/.jdks/dragonwell-21.0.11/bin/javap.exe -p -classpath $bin demo.Person
& C:/Users/limy/.jdks/dragonwell-21.0.11/bin/javap.exe -p -classpath $bin 'demo.Person$PersonBuilder'
```

`javap -p -classpath bin demo.Person` must show:

```
private java.lang.String name;
private int age;
private static final java.lang.String log;
public demo.Person();
public demo.Person(java.lang.String, int);
public static demo.Person of(java.lang.String);
public java.lang.String getName();
public void setName(java.lang.String);
public int getAge();
public java.lang.String mirrorField();
public static demo.Person$PersonBuilder builder();
```

and `demo.Person$PersonBuilder` the nested `name(String)` / `build()` pair.
